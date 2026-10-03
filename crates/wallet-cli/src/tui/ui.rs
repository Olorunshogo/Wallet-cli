//! Drawing the whole interface from [`App`] state. Pure: the same state and
//! time always draw the same frame, which the render tests rely on.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Tabs};

use super::anim;
use super::app::{App, Phase, SyncState};
use super::chain::NodeMode;
use super::format;
use super::message::Connection;
use super::theme::Tone;
use super::widgets::{self, centered, hint_line};

/// Width of the notification column.
const TOAST_WIDTH: u16 = 52;
/// How many notifications are shown at once.
const MAX_TOASTS: usize = 4;

/// Draw one frame.
pub fn draw(f: &mut Frame, app: &App, now: Instant) {
    let area = f.area();
    // Paint the dark background once; everything else draws on top of it.
    f.render_widget(Block::new().style(app.theme.base()), area);
    let [header, rest] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    draw_header(f, header, app, now);

    match &app.phase {
        Phase::Connecting => busy(
            f,
            rest,
            app,
            now,
            match app.node {
                NodeMode::External => "Connecting to the node…",
                NodeMode::Polar => "Connecting to the Polar node…",
                NodeMode::Local if app.demo => "Starting a throwaway regtest bitcoind…",
                NodeMode::Local => {
                    "Starting the local regtest bitcoind (first start can take a moment)…"
                }
                NodeMode::None => "Opening offline…",
            },
        ),
        Phase::Opening => busy(f, rest, app, now, "Opening wallet…"),
        Phase::Pick(modal) => modal.render(f, rest, &app.view(now)),
        Phase::Unlock(unlock) => unlock.render(f, rest, &app.theme),
        Phase::Onboarding(onboarding) => {
            onboarding.render(f, rest, &app.theme, now, app.loading_since())
        }
        Phase::Fatal(error) => {
            let theme = &app.theme;
            let inner = widgets::modal(f, centered(rest, 70, 9), &error.title, Tone::Error, theme);
            let lines = vec![
                Line::from(Span::styled(error.hint.clone(), theme.text())),
                Line::raw(""),
                hint_line(&[("q".into(), "quit".into())], theme),
            ];
            f.render_widget(widgets::paragraph(lines), inner);
        }
        Phase::Ready => draw_ready(f, rest, app, now),
    }
    if app.confirm_copy.is_some() {
        draw_copy_confirm(f, rest, app);
    }
    draw_toasts(f, rest, app, now);
}

fn busy(f: &mut Frame, area: Rect, app: &App, now: Instant, text: &str) {
    let theme = &app.theme;
    let line = Line::from(vec![
        Span::styled(
            format!("{} ", anim::spinner(app.loading_since(), now)),
            theme.title(),
        ),
        Span::styled(text.to_string(), theme.text()),
    ]);
    let box_area = centered(area, 56, 3);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.border(false));
    f.render_widget(
        Paragraph::new(line)
            .block(block)
            .alignment(Alignment::Center),
        box_area,
    );
}

fn draw_header(f: &mut Frame, area: Rect, app: &App, now: Instant) {
    let theme = &app.theme;
    let mut spans = vec![
        Span::styled(
            " ₿ wallet ",
            Style::new()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        ),
        Span::raw(" "),
        Span::styled(
            format!(" {} ", app.network.to_string().to_uppercase()),
            theme.network(app.network),
        ),
    ];
    if let Some(name) = &app.wallet_name {
        spans.push(Span::styled(format!(" · {name}"), theme.title()));
    }
    if app.demo {
        spans.push(Span::styled(
            " DEMO ",
            Style::new().fg(theme.info).add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(s) = &app.snapshot {
        spans.push(Span::styled(
            format!("  tip {}", format::thousands(u64::from(s.tip.height))),
            theme.text(),
        ));
    }
    spans.push(Span::raw("  "));
    spans.extend(sync_status(app, now));
    if app.outbox > 0 {
        spans.push(Span::styled(
            format!("  ✉ {} saved", app.outbox),
            Style::new().fg(theme.warning).add_modifier(Modifier::BOLD),
        ));
    }
    let right = connection_badge(app);
    let [left, right_area] = Layout::horizontal([
        Constraint::Min(10),
        Constraint::Length(right.width() as u16),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(Line::from(spans)), left);
    f.render_widget(Paragraph::new(right), right_area);
}

/// "Copy the recovery words?" Asked every time, on every network.
fn draw_copy_confirm(f: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let inner = widgets::modal(
        f,
        centered(area, 64, 9),
        "Copy recovery words?",
        Tone::Warning,
        theme,
    );
    let lines = vec![
        Line::from(Span::styled(
            "Anyone who sees your clipboard can take the coins. Other apps and \
             clipboard history can read it.",
            theme.text(),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            format!(
                "It is cleared after {} s, or when you quit.",
                app.config.clipboard_clear_secs
            ),
            theme.muted(),
        )),
        hint_line(
            &[
                ("y".into(), "copy".into()),
                ("n/esc".into(), "cancel".into()),
            ],
            theme,
        ),
    ];
    f.render_widget(widgets::paragraph(lines), inner);
}

/// ONLINE / OFFLINE / NO NODE, right-aligned in the header.
fn connection_badge(app: &App) -> Span<'static> {
    let theme = &app.theme;
    let badge = |text: String, color| {
        Span::styled(text, Style::new().fg(color).add_modifier(Modifier::BOLD))
    };
    match &app.connection {
        None => badge("… connecting ".into(), theme.muted),
        Some(Connection::Online { label, .. }) => {
            badge(format!("● ONLINE · {label} "), theme.success)
        }
        Some(Connection::Offline { retrying: true, .. }) => {
            badge("○ OFFLINE · retrying ".into(), theme.warning)
        }
        Some(Connection::Offline {
            retrying: false, ..
        }) => badge("○ NO NODE · offline mode ".into(), theme.warning),
    }
}

/// Spans describing the sync state for the header.
pub fn sync_status(app: &App, now: Instant) -> Vec<Span<'static>> {
    let theme = &app.theme;
    if !app.online() && !matches!(app.sync, SyncState::Running { .. }) {
        return match &app.snapshot {
            Some(_) => vec![Span::styled("data as of last sync", theme.muted())],
            None => Vec::new(),
        };
    }
    match app.sync {
        SyncState::Running {
            since, done, total, ..
        } => {
            let mut spans = vec![
                Span::styled(format!("{} ", anim::spinner(since, now)), theme.title()),
                Span::styled("syncing", theme.text()),
            ];
            if total > 0 {
                let width = 12;
                let filled = (u64::from(done.min(total)) * width / u64::from(total)) as usize;
                spans.push(Span::styled(format!(" {done}/{total} "), theme.muted()));
                spans.push(Span::styled(
                    "━".repeat(filled),
                    Style::new().fg(theme.accent),
                ));
                spans.push(Span::styled(
                    "━".repeat(width as usize - filled),
                    Style::new().fg(theme.skeleton),
                ));
            }
            spans
        }
        SyncState::Idle { failed: true, .. } => vec![Span::styled(
            "▲ node unreachable, retrying",
            Style::new().fg(theme.error),
        )],
        SyncState::Idle {
            last: Some(last), ..
        } => vec![Span::styled(
            format!(
                "● synced {}",
                format::ago(now.saturating_duration_since(last))
            ),
            Style::new().fg(theme.success),
        )],
        SyncState::Idle { last: None, .. } => vec![Span::styled("○ not synced yet", theme.muted())],
    }
}

fn draw_ready(f: &mut Frame, area: Rect, app: &App, now: Instant) {
    let theme = &app.theme;
    let [tabs, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(area);

    let titles: Vec<Line> = app
        .screens
        .iter()
        .enumerate()
        .map(|(i, s)| Line::from(format!("{} {}", i + 1, s.title())))
        .collect();
    f.render_widget(
        Tabs::new(titles)
            .select(app.active)
            .style(theme.muted())
            .highlight_style(theme.active_tab())
            .divider(Span::styled("│", theme.border(false))),
        tabs,
    );

    let view = app.view(now);
    app.screens[app.active].render(f, body, &view);
    f.render_widget(Paragraph::new(hint_line(&app.hints(), theme)), footer);

    if let Some(modal) = &app.modal {
        modal.render(f, area, &view);
    }
}

fn draw_toasts(f: &mut Frame, area: Rect, app: &App, now: Instant) {
    let theme = &app.theme;
    let ttl = Duration::from_secs(app.config.toast_secs);
    let width = TOAST_WIDTH.min(area.width);
    // Bottom-right, newest lowest, stacking upward above the footer, so they
    // stay clear of panel titles and the send review at the top.
    let mut bottom = area.bottom().saturating_sub(1);
    for toast in app.toasts.iter().rev().take(MAX_TOASTS) {
        let life = anim::lifecycle(toast.created, ttl, now);
        if life.expired || bottom < area.y + 3 {
            continue;
        }
        // Slide in from the right edge.
        let visible = ((f64::from(width) * life.shown).round() as u16).max(1);
        let rect = Rect {
            x: area.right().saturating_sub(visible + 1),
            y: bottom - 3,
            width: visible,
            height: 3,
        };
        let (icon, color) = match toast.tone {
            Tone::Success => ("✔", theme.success),
            Tone::Info => ("ℹ", theme.info),
            Tone::Warning => ("▲", theme.warning),
            Tone::Error => ("✗", theme.error),
        };
        f.render_widget(Clear, rect);
        f.render_widget(Block::new().style(theme.base()), rect);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(color));
        let text = Line::from(vec![
            Span::styled(
                format!("{icon} "),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(toast.text.clone(), theme.text()),
        ]);
        f.render_widget(Paragraph::new(text).block(block), rect);
        bottom -= 3;
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use wallet::bitcoin::hashes::Hash;
    use wallet::bitcoin::{Amount, BlockHash, Network, Txid};
    use wallet::{Balance, BlockId, KeySource, MnemonicLength, SyncReport, TxDetails, TxStatus};

    use super::*;
    use crate::session::StoredKeys;
    use crate::tui::app::StartupWallet;
    use crate::tui::config::TuiConfig;
    use crate::tui::format::ErrorView;
    use crate::tui::message::{Connection, Snapshot, WorkerEvent};
    use crate::tui::theme::Theme;

    fn text(buffer: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn render(app: &App, now: Instant) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
        terminal.draw(|f| draw(f, app, now)).unwrap();
        text(terminal.backend().buffer())
    }

    fn snapshot(confirmed: u64, txs: Vec<TxDetails>) -> Snapshot {
        let phrase = wallet::generate_mnemonic(MnemonicLength::Words12).unwrap();
        let mut w = wallet::Wallet::builder(Network::Regtest)
            .keys(KeySource::mnemonic(phrase.as_str()))
            .create()
            .unwrap();
        Snapshot {
            network: Network::Regtest,
            can_sign: true,
            tip: BlockId {
                height: 1_234,
                hash: BlockHash::all_zeros(),
            },
            balance: Balance {
                confirmed: Amount::from_sat(confirmed),
                ..Balance::default()
            },
            utxos: Vec::new(),
            txs,
            receive: w.new_address().unwrap(),
        }
    }

    fn ready(now: Instant, synced: bool, txs: Vec<TxDetails>) -> App {
        let mut app = App::new(
            Network::Regtest,
            StartupWallet::Existing(StoredKeys::None),
            TuiConfig::default(),
            Theme::default(),
            now,
        );
        app.on_event(
            WorkerEvent::Connection(Connection::Online {
                label: "test backend".into(),
                can_mine: false,
            }),
            now,
        );
        app.on_event(WorkerEvent::Opened(snapshot(0, vec![])), now);
        if synced {
            let s = snapshot(123_456, txs);
            app.on_event(
                WorkerEvent::Synced {
                    report: SyncReport {
                        from: s.tip,
                        to: s.tip,
                        blocks_applied: 1,
                        reorg_depth: 0,
                        mempool_txs: 0,
                    },
                    snapshot: s,
                    manual: false,
                },
                now,
            );
        }
        app.toasts.clear();
        app
    }

    #[test]
    fn connecting_shows_a_spinner_and_the_network() {
        let now = Instant::now();
        let app = App::new(
            Network::Signet,
            StartupWallet::Missing,
            TuiConfig::default(),
            Theme::default(),
            now,
        );
        let screen = render(&app, now);
        assert!(screen.contains("Connecting to the node"));
        assert!(screen.contains("SIGNET"));
        assert!(screen.contains(anim::SPINNER[0]));
    }

    #[test]
    fn dashboard_shows_skeletons_before_the_first_sync() {
        let now = Instant::now();
        let screen = render(&ready(now, false, vec![]), now);
        assert!(screen.contains("Confirmed"));
        assert!(screen.contains("█"), "skeleton bars");
        assert!(!screen.contains("123,456"));
    }

    #[test]
    fn dashboard_shows_balances_and_activity_after_sync() {
        let now = Instant::now();
        let tx = TxDetails {
            txid: Txid::from_byte_array([7; 32]),
            sent: Amount::ZERO,
            received: Amount::from_sat(123_456),
            fee: None,
            status: TxStatus::Confirmed {
                height: 1_200,
                confirmations: 35,
            },
        };
        let app = ready(now, true, vec![tx]);
        let screen = render(&app, now + Duration::from_secs(5));
        assert!(screen.contains("123,456 sat"), "{screen}");
        assert!(screen.contains("+123,456"));
        assert!(screen.contains("35 conf"));
        assert!(screen.contains("tip 1,234"));
        assert!(screen.contains("synced"));
        assert!(screen.contains("test backend"));
    }

    #[test]
    fn every_screen_renders() {
        let now = Instant::now();
        let mut app = ready(now, true, vec![]);
        for (i, expected) in [
            "Confirmed",
            "Address #0",
            "New payment",
            "No transactions",
            "No coins",
        ]
        .iter()
        .enumerate()
        {
            app.active = i;
            let screen = render(&app, now);
            assert!(
                screen.contains(expected),
                "screen {i} missing {expected:?}:\n{screen}"
            );
        }
    }

    #[test]
    fn toasts_and_fatal_errors_are_drawn() {
        let now = Instant::now();
        let mut app = ready(now, true, vec![]);
        app.notify(Tone::Success, "Received 1,000 sat", now);
        let screen = render(&app, now + Duration::from_millis(500));
        assert!(screen.contains("Received 1,000 sat"));

        app.phase = Phase::Fatal(ErrorView::new("Could not connect", "Is bitcoind running?"));
        let screen = render(&app, now);
        assert!(screen.contains("Could not connect"));
        assert!(screen.contains("Is bitcoind running?"));
    }

    #[test]
    fn long_history_is_paginated_to_fit_the_screen() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let now = Instant::now();
        let txs: Vec<TxDetails> = (0..50u8)
            .map(|i| TxDetails {
                txid: Txid::from_byte_array([i; 32]),
                sent: Amount::ZERO,
                received: Amount::from_sat(1_000 + u64::from(i)),
                fee: None,
                status: TxStatus::Confirmed {
                    height: 100,
                    confirmations: 1,
                },
            })
            .collect();
        let mut app = ready(now, true, txs);
        app.on_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE), now);
        let first = render(&app, now);
        assert!(first.contains("page 1 of"), "{first}");
        assert!(first.contains(&Txid::from_byte_array([0; 32]).to_string()));
        assert!(
            !first.contains(&Txid::from_byte_array([49; 32]).to_string()),
            "last tx is on a later page"
        );

        app.on_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE), now);
        let second = render(&app, now);
        assert!(second.contains("page 2 of"), "{second}");
        assert!(!second.contains(&Txid::from_byte_array([0; 32]).to_string()));

        app.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE), now);
        let last = render(&app, now);
        assert!(last.contains("of 50"));
        assert!(last.contains(&Txid::from_byte_array([49; 32]).to_string()));
    }

    #[test]
    fn sync_progress_bar_in_header() {
        let now = Instant::now();
        let mut app = ready(now, true, vec![]);
        app.sync = SyncState::Running {
            since: now,
            manual: true,
            done: 6,
            total: 12,
        };
        let screen = render(&app, now);
        assert!(screen.contains("syncing 6/12"));
        assert!(screen.contains("━"));
    }
}
