//! Interactive terminal interface: `wallet-cli tui`.
//!
//! Layers, from the outside in:
//! - `mod.rs`: terminal setup and the event loop (the only code doing IO).
//! - [`app`]: the state machine; [`ui`] draws it.
//! - [`screens`], [`startup`], [`modal`]: the views, each in its own file.
//! - [`worker`]: owns the wallet on a background thread; talks to the UI
//!   through [`message`] types only.
//! - [`chain`]: Bitcoin Core, the demo node, or test doubles.
//! - [`config`], [`theme`], [`keymap`]: every tunable value and color.
//! - [`validate`], [`widgets`], [`format`], [`anim`]: reusable building blocks.

pub mod anim;
pub mod app;
pub mod chain;
pub mod clipboard;
pub mod config;
pub mod format;
pub mod keymap;
pub mod message;
pub mod modal;
pub mod screens;
pub mod startup;
pub mod theme;
pub mod ui;
pub mod validate;
pub mod widgets;
pub mod worker;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use tui_big_text::{BigText, PixelSize};
use wallet::bitcoin::{FeeRate, Network};

use self::app::{App, StartupWallet};
use self::chain::NodeMode;
use self::config::TuiConfig;
use self::theme::Theme;
use self::worker::{WorkerConfig, WorkerHandle, WorkerSpec};
use crate::config::{DataDir, RpcArgs};
use crate::node::LocalNode;
use crate::session;
use crate::wallets::Wallets;

/// Options for `wallet-cli tui`.
#[derive(clap::Args, Debug, Clone)]
pub struct TuiArgs {
    /// Start a throwaway regtest node and wallet; nothing to set up.
    /// Overrides `--node`.
    #[arg(long)]
    pub demo: bool,
    /// TUI settings file. Defaults to `<datadir>/tui.toml`.
    #[arg(long)]
    pub tui_config: Option<PathBuf>,
    /// Word count new wallets start with (12, 15, 18, 21 or 24; changeable
    /// on the setup screen). Overrides `mnemonic_words` in `tui.toml`.
    #[arg(long, env = "WALLET_WORDS", value_parser = crate::parse_word_count)]
    pub words: Option<usize>,
    /// Address type new wallets start with (changeable on the setup screen).
    /// Overrides `address_kind` in `tui.toml`.
    #[arg(long, value_enum, env = "WALLET_ADDRESS_TYPE")]
    pub address_type: Option<crate::AddressType>,
}

/// Which wallet the TUI opens, and whether it can switch to others.
pub struct Target {
    /// The wallet's directory.
    pub dir: DataDir,
    /// Its name, for named wallets.
    pub name: Option<String>,
    /// All named wallets; `None` with `--datadir` (no switching).
    pub wallets: Option<Wallets>,
    /// No wallet was named and `dir` holds none: ask which one to open, or
    /// what to call a new one, instead of creating `dir`.
    pub pick: bool,
}

/// Run the TUI until the user quits. Quitting stops everything the TUI
/// started: its worker, and the local node unless another program uses it.
pub fn run(
    args: TuiArgs,
    network: Network,
    node: NodeMode,
    target: Target,
    rpc: RpcArgs,
    local: LocalNode,
) -> Result<()> {
    // While picking, nothing belongs to a wallet yet, so settings and logs
    // live next to the wallets instead of in a directory that may never be
    // used.
    let target = match &target.wallets {
        Some(wallets) if target.pick => Target {
            dir: DataDir::at(wallets.root().to_path_buf()),
            name: None,
            ..target
        },
        _ => target,
    };
    let config_path = args
        .tui_config
        .clone()
        .unwrap_or_else(|| target.dir.path().join("tui.toml"));
    let mut config = TuiConfig::load(&config_path)?;
    if let Some(words) = args.words {
        config.mnemonic_words = words;
    }
    if let Some(address_type) = args.address_type {
        config.address_kind = address_type.into();
    }
    let theme = config.theme.build().map_err(anyhow::Error::msg)?;

    // Demo mode works in a temporary directory that disappears on exit;
    // `_keep_alive` holds it until the TUI closes.
    let mut _keep_alive: Option<Box<dyn std::any::Any>> = None;
    let node = if args.demo { NodeMode::Local } else { node };
    if matches!(node, NodeMode::Local | NodeMode::Polar) && network != Network::Regtest {
        bail!("a local node only runs regtest; drop --network or use --node external");
    }
    let mut fallback = None;
    let (target, startup, connect) = if args.demo {
        let (tmp, connect) = throwaway_node()?;
        let dir = DataDir::at(tmp.path().join("wallet"));
        _keep_alive = Some(Box::new(tmp));
        let target = Target {
            dir,
            name: Some("demo".into()),
            wallets: None,
            pick: false,
        };
        (target, StartupWallet::Demo, Some(connect))
    } else {
        let startup = if target.pick && target.wallets.is_some() {
            StartupWallet::Pick
        } else if session::exists(&target.dir) {
            StartupWallet::Existing(session::stored_keys(&target.dir)?)
        } else {
            StartupWallet::Missing
        };
        let connect = match node {
            NodeMode::External => Some(chain::rpc(rpc, network)),
            NodeMode::Local => Some(chain::shared(local)),
            NodeMode::Polar => {
                fallback = Some(worker::Fallback {
                    connect: chain::shared(local),
                    wallets: Wallets::new(None, Network::Regtest),
                });
                Some(chain::polar(rpc))
            }
            NodeMode::None => None,
        };
        (target, startup, connect)
    };

    target.dir.ensure()?;
    init_file_logging(&target.dir)?;

    let can_switch = target.wallets.is_some();
    let wallet_name = target.name.clone();
    let worker = WorkerHandle::spawn(WorkerSpec {
        network,
        dir: target.dir,
        name: target.name,
        wallets: target.wallets,
        config: WorkerConfig {
            fallback_fee_rate: FeeRate::from_sat_per_vb(config.fallback_fee_rate)
                .context("fallback_fee_rate is too large")?,
            progress_every: Duration::from_millis(50),
        },
        connect,
        fallback,
    });
    let frame = config.frame();
    let mut app = App::new(network, startup, config, theme, Instant::now());
    app.node = node;
    app.wallet_name = wallet_name;
    app.can_switch = can_switch;
    app.can_fallback = node == NodeMode::Polar;

    let mut terminal = ratatui::init();
    let result = splash(&mut terminal, &app.theme)
        .and_then(|()| event_loop(&mut terminal, &mut app, &worker, frame));
    ratatui::restore();
    if node == NodeMode::Local {
        eprintln!("Closing… (stopping the local node if nothing else uses it)");
    }
    // Dropping the handle stops the worker, which releases the local node.
    drop(worker);
    result
}

/// Exact rendered width of "B" at `PixelSize::Full` and of
/// "WALLET"/"LIBRARY" at `PixelSize::HalfHeight`, measured with a
/// throwaway `TestBackend` render. Used to slide each in from off its
/// final spot.
const BADGE_WIDTH: u16 = 7;
const WALLET_WIDTH: u16 = 46;
const LIBRARY_WIDTH: u16 = 54;
const GLYPH_HEIGHT: u16 = 4;
/// Height of "B" at `PixelSize::Full`: every row is a solid block (no
/// half-filled top/bottom row like `HalfHeight` has), so a tick mark
/// touches it flush on both sides instead of looking closer on one end.
const BADGE_GLYPH_HEIGHT: u16 = 7;

/// Column (and width) of "B"'s left vertical stroke within its own
/// `BADGE_WIDTH`-wide box (measured the same way), where the tick marks
/// line up - matching the stroke's own weight - to make it read as ₿
/// rather than a plain letter.
const BADGE_TICK_COL: u16 = 1;
const BADGE_TICK_WIDTH: u16 = 2;
/// One row of tick above the glyph, one below.
const BADGE_HEIGHT: u16 = 1 + BADGE_GLYPH_HEIGHT + 1;

/// One-time animated banner shown before the normal startup flow. A ₿ badge
/// (a big "B" with a tick mark through the top and bottom, evoking the
/// Bitcoin symbol without needing a Unicode glyph the font doesn't have)
/// slides down from the top, "WALLET" slides in from the left and "LIBRARY"
/// from the right, the tagline types itself in below, then "Press any key
/// to continue" blinks until a key is pressed (any key skips straight to
/// the usual picker/unlock/onboarding).
///
/// Everything here is block characters rendered through [`tui_big_text`] -
/// the same technique throughout - so it looks identical in every terminal,
/// with no image-protocol detection or resizing involved.
fn splash(terminal: &mut DefaultTerminal, theme: &Theme) -> Result<()> {
    const TAGLINE: &str = "A modular Bitcoin wallet library wrapping BDK.";
    const WALLET_DELAY: Duration = Duration::from_millis(250);
    const LIBRARY_DELAY: Duration = Duration::from_millis(400);
    const SLIDE_IN: Duration = Duration::from_millis(400);
    const TAGLINE_DELAY: Duration = Duration::from_millis(800);
    const TYPE_IN: Duration = Duration::from_millis(500);
    const PROMPT_DELAY: Duration = Duration::from_millis(1_500);
    const BLINK: Duration = Duration::from_millis(500);

    let accent = Style::new().fg(theme.accent).add_modifier(Modifier::BOLD);
    let plain = Style::new().fg(theme.text).add_modifier(Modifier::BOLD);
    let badge_text = BigText::builder()
        .pixel_size(PixelSize::Full)
        .style(accent)
        .lines(vec!["B".into()])
        .build();
    let wallet_text = BigText::builder()
        .pixel_size(PixelSize::HalfHeight)
        .style(plain)
        .lines(vec!["WALLET".into()])
        .build();
    let library_text = BigText::builder()
        .pixel_size(PixelSize::HalfHeight)
        .style(accent)
        .lines(vec!["LIBRARY".into()])
        .build();

    let start = Instant::now();
    loop {
        let elapsed = Instant::now().saturating_duration_since(start);

        let tagline = typed_in(TAGLINE, elapsed, TAGLINE_DELAY, TYPE_IN);
        let blink_on = (elapsed.as_millis() / BLINK.as_millis()).is_multiple_of(2);
        let prompt = if elapsed >= PROMPT_DELAY && blink_on {
            "Press any key to continue"
        } else {
            ""
        };

        terminal.draw(|f| {
            f.render_widget(Block::new().style(theme.base()), f.area());
            let term_w = f.area().width;

            let total_h = BADGE_HEIGHT + 1 + GLYPH_HEIGHT + 1 + GLYPH_HEIGHT + 1 + 1 + 1 + 1;
            let top = f.area().y + f.area().height.saturating_sub(total_h) / 2;
            let mut y = top;

            let badge_x = f.area().x + (term_w.saturating_sub(BADGE_WIDTH)) / 2;
            let badge_y = slide(f.area().y, top, elapsed, Duration::ZERO, SLIDE_IN);
            let tick_x = badge_x + BADGE_TICK_COL;
            f.render_widget(
                Span::styled("██", accent),
                Rect::new(tick_x, badge_y, BADGE_TICK_WIDTH, 1),
            );
            f.render_widget(
                &badge_text,
                Rect::new(badge_x, badge_y + 1, BADGE_WIDTH, BADGE_GLYPH_HEIGHT),
            );
            f.render_widget(
                Span::styled("██", accent),
                Rect::new(
                    tick_x,
                    badge_y + 1 + BADGE_GLYPH_HEIGHT,
                    BADGE_TICK_WIDTH,
                    1,
                ),
            );
            y += BADGE_HEIGHT + 1;

            let wallet_x = slide(
                f.area().x,
                f.area().x + (term_w.saturating_sub(WALLET_WIDTH)) / 2,
                elapsed,
                WALLET_DELAY,
                SLIDE_IN,
            );
            f.render_widget(
                &wallet_text,
                Rect::new(wallet_x, y, WALLET_WIDTH, GLYPH_HEIGHT),
            );
            y += GLYPH_HEIGHT + 1;

            let library_x = slide(
                f.area().x + term_w.saturating_sub(LIBRARY_WIDTH),
                f.area().x + (term_w.saturating_sub(LIBRARY_WIDTH)) / 2,
                elapsed,
                LIBRARY_DELAY,
                SLIDE_IN,
            );
            f.render_widget(
                &library_text,
                Rect::new(library_x, y, LIBRARY_WIDTH, GLYPH_HEIGHT),
            );
            y += GLYPH_HEIGHT + 1;

            let tagline_row = Rect::new(f.area().x, y, term_w, 1);
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(tagline, theme.text())))
                    .alignment(Alignment::Center),
                tagline_row,
            );
            y += 2;

            let prompt_row = Rect::new(f.area().x, y, term_w, 1);
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(prompt, theme.muted())))
                    .alignment(Alignment::Center),
                prompt_row,
            );
        })?;
        if event::poll(Duration::from_millis(33))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            return Ok(());
        }
    }
}

/// Eased position (x or y) between `from` and `to`, starting at `delay` and
/// settling `dur` later. Before `delay`, stays at `from` (off its final
/// spot, e.g. flush with a screen edge).
fn slide(from: u16, to: u16, elapsed: Duration, delay: Duration, dur: Duration) -> u16 {
    if elapsed < delay {
        return from;
    }
    let t = anim::ease_out(((elapsed - delay).as_secs_f64() / dur.as_secs_f64()).min(1.0));
    (from as f64 + (to as f64 - from as f64) * t).round() as u16
}

/// `text`, revealed character by character between `delay` and `delay + dur`
/// after the animation started (empty before `delay`, whole after).
fn typed_in(text: &str, elapsed: Duration, delay: Duration, dur: Duration) -> &str {
    if elapsed < delay {
        return "";
    }
    let t = (elapsed - delay).as_secs_f64() / dur.as_secs_f64();
    let shown = (anim::ease_out(t) * text.chars().count() as f64).round() as usize;
    match text.char_indices().nth(shown) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

#[cfg(test)]
mod splash_tests {
    use super::*;

    #[test]
    fn typed_in_is_empty_before_the_delay() {
        assert_eq!(
            typed_in(
                "hi",
                Duration::ZERO,
                Duration::from_millis(100),
                Duration::from_millis(100)
            ),
            ""
        );
    }

    #[test]
    fn typed_in_is_whole_once_done() {
        let dur = Duration::from_millis(100);
        assert_eq!(typed_in("hi", dur, Duration::ZERO, dur), "hi");
        assert_eq!(typed_in("hi", dur * 10, Duration::ZERO, dur), "hi");
    }

    #[test]
    fn typed_in_grows_with_time_and_stays_on_char_boundaries() {
        let dur = Duration::from_millis(100);
        let text = "héllo"; // é is multi-byte, to catch slicing bugs
        let early = typed_in(text, dur / 10, Duration::ZERO, dur);
        let later = typed_in(text, dur / 2, Duration::ZERO, dur);
        assert!(early.len() <= later.len());
        assert!(text.starts_with(early));
        assert!(text.starts_with(later));
    }

    #[test]
    fn slide_stays_at_from_before_the_delay() {
        let delay = Duration::from_millis(100);
        let dur = Duration::from_millis(100);
        assert_eq!(slide(0, 20, Duration::ZERO, delay, dur), 0);
        assert_eq!(slide(50, 20, Duration::from_millis(50), delay, dur), 50);
    }

    #[test]
    fn slide_reaches_to_once_done() {
        let dur = Duration::from_millis(100);
        assert_eq!(slide(0, 20, dur, Duration::ZERO, dur), 20);
        assert_eq!(slide(0, 20, dur * 10, Duration::ZERO, dur), 20);
    }

    #[test]
    fn slide_moves_monotonically_toward_to() {
        let dur = Duration::from_millis(100);
        let early = slide(0, 20, dur / 4, Duration::ZERO, dur);
        let later = slide(0, 20, dur / 2, Duration::ZERO, dur);
        assert!(early <= later);
        assert!(later <= 20);
    }
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    worker: &WorkerHandle,
    frame: Duration,
) -> Result<()> {
    let mut clipboard = clipboard::SystemClipboard::default();
    loop {
        let now = Instant::now();
        while let Some(event) = worker.try_recv() {
            for command in app.on_event(event, now) {
                worker.send(command);
            }
        }
        for command in app.on_tick(now) {
            worker.send(command);
        }
        app.run_clipboard(&mut clipboard, now);
        terminal.draw(|f| ui::draw(f, app, now))?;
        if app.should_quit {
            app.on_quit();
            app.run_clipboard(&mut clipboard, now);
            return Ok(());
        }
        if event::poll(frame)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            for command in app.on_key(key, Instant::now()) {
                worker.send(command);
            }
        }
    }
}

/// A throwaway regtest node for `--demo`.
#[cfg(feature = "local-node")]
fn throwaway_node() -> Result<(tempfile::TempDir, chain::ChainFactory)> {
    Ok((tempfile::tempdir()?, chain::throwaway()))
}

#[cfg(not(feature = "local-node"))]
fn throwaway_node() -> Result<(NoDemo, chain::ChainFactory)> {
    bail!("this build cannot run a demo node; rebuild with the `local-node` feature")
}

/// Stand-in so the demo branch type-checks when the feature is off.
#[cfg(not(feature = "local-node"))]
struct NoDemo;

#[cfg(not(feature = "local-node"))]
impl NoDemo {
    fn path(&self) -> &std::path::Path {
        std::path::Path::new("")
    }
}

/// Logs would corrupt the screen, so the TUI writes them to `tui.log`.
fn init_file_logging(dir: &DataDir) -> Result<()> {
    let path = dir.path().join("tui.log");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("could not open {}", path.display()))?;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .try_init();
    Ok(())
}

/// Drives the real app, worker thread and a real regtest `bitcoind` with
/// key presses, the way a user would, and checks what is drawn.
#[cfg(all(test, feature = "local-node"))]
mod end_to_end {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use wallet::bitcoin::Amount;
    use wallet::{KeySource, MnemonicLength, TxStatus, Wallet};

    use super::*;

    struct Session {
        app: App,
        worker: WorkerHandle,
        _tmp: tempfile::TempDir,
    }

    /// A port nothing listens on right now (tests run in parallel).
    fn free_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    impl Session {
        fn demo() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let dir = DataDir::at(tmp.path().join("w"));
            Self::start(
                tmp,
                dir,
                StartupWallet::Demo,
                Some(chain::throwaway()),
                None,
            )
        }

        /// Named wallets under `root`, nothing named yet: starts at the
        /// picker, like `wallet-cli --node local tui` on a fresh checkout.
        fn named(root: tempfile::TempDir) -> Self {
            let wallets = Wallets::new(Some(root.path().join("home")), Network::Regtest);
            let dir = DataDir::at(wallets.root().to_path_buf());
            let connect = chain::shared(LocalNode {
                dir: wallets.root().join("node"),
                port: free_port(),
            });
            Self::start(root, dir, StartupWallet::Pick, Some(connect), Some(wallets))
        }

        /// `--node local` on `root`: node data in `root/w/node`, persistent.
        fn local(root: tempfile::TempDir) -> Self {
            let dir = DataDir::at(root.path().join("w"));
            let startup = if session::exists(&dir) {
                StartupWallet::Existing(session::stored_keys(&dir).unwrap())
            } else {
                StartupWallet::Missing
            };
            let connect = chain::shared(LocalNode {
                dir: dir.path().join("node"),
                port: free_port(),
            });
            Self::start(root, dir, startup, Some(connect), None)
        }

        fn start(
            tmp: tempfile::TempDir,
            dir: DataDir,
            startup: StartupWallet,
            connect: Option<chain::ChainFactory>,
            wallets: Option<Wallets>,
        ) -> Self {
            let can_switch = wallets.is_some();
            let config = TuiConfig {
                refresh_secs: 0,
                toast_secs: 1,
                ..TuiConfig::default()
            };
            let worker = WorkerHandle::spawn(WorkerSpec {
                network: Network::Regtest,
                dir,
                name: None,
                wallets,
                config: WorkerConfig {
                    fallback_fee_rate: FeeRate::from_sat_per_vb_u32(2),
                    progress_every: Duration::ZERO,
                },
                connect,
                fallback: None,
            });
            let mut app = App::new(
                Network::Regtest,
                startup,
                config,
                theme::Theme::default(),
                Instant::now(),
            );
            app.can_switch = can_switch;
            Self {
                app,
                worker,
                _tmp: tmp,
            }
        }

        /// Exchange messages until `done` holds, or fail after a timeout.
        fn pump_until(&mut self, what: &str, done: impl Fn(&App) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(60);
            while !done(&self.app) {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for: {what}\n{}",
                    self.screen()
                );
                if let Some(event) = self.worker.try_recv() {
                    for c in self.app.on_event(event, Instant::now()) {
                        self.worker.send(c);
                    }
                } else {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }

        fn press(&mut self, code: KeyCode) {
            for c in self
                .app
                .on_key(KeyEvent::new(code, KeyModifiers::NONE), Instant::now())
            {
                self.worker.send(c);
            }
        }

        fn press_ctrl(&mut self, ch: char) {
            for c in self.app.on_key(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL),
                Instant::now(),
            ) {
                self.worker.send(c);
            }
        }

        /// Run the setup screen with its defaults: new words, no password.
        fn create_wallet(&mut self, name: &str) {
            self.pump_until("setup screen", |a| {
                matches!(a.phase, app::Phase::Onboarding(_))
            });
            assert_eq!(self.app.wallet_name.as_deref(), Some(name));
            self.press(KeyCode::Enter); // create a new wallet
            self.press(KeyCode::Enter); // words written down
            self.press(KeyCode::Enter); // no password
            self.pump_until("new wallet synced", |a| {
                matches!(a.phase, app::Phase::Ready) && a.synced
            });
        }

        /// Open the switcher and wait for its list.
        fn open_switcher(&mut self) {
            self.press(KeyCode::Char('w'));
            self.pump_until("wallet list", |a| {
                matches!(&a.modal, Some(modal::Modal::Wallets { list: Some(_), .. }))
            });
        }

        fn type_text(&mut self, text: &str) {
            for ch in text.chars() {
                self.press(KeyCode::Char(ch));
            }
        }

        fn screen(&self) -> String {
            let mut terminal = Terminal::new(TestBackend::new(130, 38)).unwrap();
            terminal
                .draw(|f| ui::draw(f, &self.app, Instant::now()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let mut out = String::new();
            for y in 0..buffer.area.height {
                for x in 0..buffer.area.width {
                    out.push_str(buffer[(x, y)].symbol());
                }
                out.push('\n');
            }
            out
        }

        /// Let notifications expire so the screen underneath is readable.
        fn settle(&mut self) {
            std::thread::sleep(Duration::from_millis(1_100));
            for c in self.app.on_tick(Instant::now()) {
                self.worker.send(c);
            }
        }

        fn confirmed(&self) -> Amount {
            self.app
                .snapshot
                .as_ref()
                .map_or(Amount::ZERO, |s| s.balance.confirmed)
        }
    }

    #[test]
    fn demo_faucet_send_and_confirm_through_the_ui() {
        let mut s = Session::demo();
        s.pump_until("wallet ready and synced", |a| {
            matches!(a.phase, app::Phase::Ready) && a.synced
        });
        assert!(
            s.screen().contains("Demo wallet ready"),
            "welcome dialog shows fresh words"
        );
        s.press(KeyCode::Enter);

        s.press(KeyCode::Char('f'));
        s.pump_until("faucet coins", |a| {
            a.snapshot
                .as_ref()
                .is_some_and(|s| s.balance.confirmed > Amount::ZERO)
        });
        assert_eq!(
            s.confirmed(),
            Amount::from_int_btc(100),
            "two coinbases mature at height 101"
        );

        // Pay a brand-new wallet through the Send form.
        let phrase = wallet::generate_mnemonic(MnemonicLength::Words12).unwrap();
        let to = Wallet::builder(Network::Regtest)
            .keys(KeySource::mnemonic(phrase.as_str()))
            .create()
            .unwrap()
            .new_address()
            .unwrap()
            .address
            .to_string();
        s.press(KeyCode::Char('3'));
        s.pump_until("fee estimate filled in", |a| {
            a.toasts.iter().any(|t| t.text.contains("No fee data"))
        });
        s.press(KeyCode::Enter);
        s.type_text(&to);
        s.press(KeyCode::Tab);
        s.type_text("250000");
        s.press(KeyCode::Enter);
        let deadline = Instant::now() + Duration::from_secs(30);
        while !s.screen().contains("Review") {
            assert!(Instant::now() < deadline, "no review:\n{}", s.screen());
            if let Some(event) = s.worker.try_recv() {
                for c in s.app.on_event(event, Instant::now()) {
                    s.worker.send(c);
                }
            }
        }
        s.settle();
        let review = s.screen();
        assert!(review.contains("250,000 sat"), "{review}");
        assert!(review.contains("sat/vB"));

        s.press(KeyCode::Char('y'));
        s.pump_until("sent", |a| {
            a.toasts
                .iter()
                .any(|t| t.text.starts_with("Sent 250,000 sat"))
        });
        let pending = s.app.snapshot.as_ref().unwrap().txs[0].clone();
        assert_eq!(pending.status, TxStatus::Unconfirmed);

        // Mine a block from the UI and watch it confirm.
        s.press(KeyCode::Char('m'));
        s.pump_until("confirmed", |a| {
            a.snapshot
                .as_ref()
                .unwrap()
                .txs
                .iter()
                .any(|t| t.txid == pending.txid && t.status.is_confirmed())
        });
        s.press(KeyCode::Char('4'));
        s.settle();
        let history = s.screen();
        assert!(history.contains(&pending.txid.to_string()), "{history}");
        assert!(history.contains("1 conf"));
    }

    #[test]
    fn two_named_wallets_pay_each_other_through_the_ui() {
        let mut s = Session::named(tempfile::tempdir().unwrap());

        // Nothing exists yet, so the picker asks for a name right away.
        s.pump_until("name prompt", |a| {
            matches!(
                &a.phase,
                app::Phase::Pick(modal::Modal::Wallets {
                    naming: Some(_),
                    ..
                })
            )
        });
        s.type_text("Alice");
        s.press(KeyCode::Enter);
        s.create_wallet("alice");
        s.press(KeyCode::Char('f'));
        s.pump_until("faucet coins", |a| {
            a.snapshot
                .as_ref()
                .is_some_and(|s| s.balance.confirmed > Amount::ZERO)
        });

        // A second wallet, named in the switcher.
        s.open_switcher();
        s.press(KeyCode::Char('n'));
        s.type_text("bob");
        s.press(KeyCode::Enter);
        s.create_wallet("bob");
        assert_eq!(s.confirmed(), Amount::ZERO);

        // Back to alice (the list is sorted, bob is highlighted).
        s.open_switcher();
        s.press(KeyCode::Up);
        s.press(KeyCode::Enter);
        s.pump_until("alice open with her coins", |a| {
            a.wallet_name.as_deref() == Some("alice")
                && a.synced
                && a.snapshot
                    .as_ref()
                    .is_some_and(|s| s.balance.confirmed > Amount::ZERO)
        });

        // Pay bob by name: ctrl+w fills in his address.
        s.press(KeyCode::Char('3'));
        s.pump_until("fee estimate filled in", |a| {
            a.toasts.iter().any(|t| t.text.contains("No fee data"))
        });
        s.press(KeyCode::Enter);
        s.press_ctrl('w');
        s.pump_until("bob's address", |a| {
            a.toasts
                .iter()
                .any(|t| t.text.starts_with("Paying wallet bob"))
        });
        s.type_text("0.5btc");
        s.press(KeyCode::Enter);
        let deadline = Instant::now() + Duration::from_secs(30);
        while !s.screen().contains("Review") {
            assert!(Instant::now() < deadline, "no review:\n{}", s.screen());
            if let Some(event) = s.worker.try_recv() {
                for c in s.app.on_event(event, Instant::now()) {
                    s.worker.send(c);
                }
            }
        }
        s.press(KeyCode::Char('y'));
        s.pump_until("sent", |a| {
            a.toasts
                .iter()
                .any(|t| t.text.starts_with("Sent 50,000,000 sat"))
        });
        s.press(KeyCode::Char('m'));
        s.pump_until("mined", |a| {
            a.toasts.iter().any(|t| t.text.starts_with("Mined 1 block"))
        });

        // Bob sees it confirmed.
        s.open_switcher();
        s.press(KeyCode::Down);
        s.press(KeyCode::Enter);
        s.pump_until("bob paid", |a| {
            a.wallet_name.as_deref() == Some("bob")
                && a.synced
                && a.snapshot
                    .as_ref()
                    .is_some_and(|s| s.balance.confirmed == Amount::from_sat(50_000_000))
        });
    }

    #[test]
    fn local_node_keeps_chain_and_coins_between_runs() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().to_path_buf();

        // First run: onboarding creates a wallet, then the faucet funds it.
        let mut s = Session::local(root);
        s.pump_until("onboarding", |a| {
            matches!(a.phase, app::Phase::Onboarding(_))
        });
        s.press(KeyCode::Enter); // create a new wallet
        s.press(KeyCode::Enter); // words written down
        s.press(KeyCode::Enter); // no password
        s.pump_until("synced", |a| {
            matches!(a.phase, app::Phase::Ready) && a.synced
        });
        assert!(s.app.online());
        s.press(KeyCode::Char('f'));
        s.pump_until("faucet coins", |a| {
            a.snapshot
                .as_ref()
                .is_some_and(|s| s.balance.confirmed > Amount::ZERO)
        });
        let funded = s.confirmed();
        let Session { app, worker, _tmp } = s;
        drop(app);
        drop(worker); // quitting releases the node
        assert!(root_path.join("w/node").exists(), "node data kept on disk");
        assert_eq!(
            crate::node::Shared::new(root_path.join("w/node")).status(),
            crate::node::Status::Stopped,
            "quitting the TUI stopped the node it started"
        );

        // Second run on the same directory: the wallet opens, the node
        // restarts on the same chain, and the coins are still there.
        let mut s = Session::local(_tmp);
        s.pump_until("synced again", |a| {
            matches!(a.phase, app::Phase::Ready) && a.synced && a.online()
        });
        s.pump_until("stored balance", |a| {
            a.snapshot
                .as_ref()
                .is_some_and(|s| s.balance.confirmed == funded)
        });
        assert!(s.app.snapshot.as_ref().unwrap().tip.height >= 101);
    }
}
