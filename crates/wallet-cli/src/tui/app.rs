//! The TUI state machine.
//!
//! [`App`] turns keys, worker events and clock ticks into state changes and
//! [`Command`]s. It does no IO and never reads the clock itself, so the whole
//! interface flow is unit-tested without a terminal or a node.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use wallet::KeySource;
use wallet::bitcoin::{Amount, Network, Txid};
use zeroize::Zeroizing;

use super::anim::{self, Tween};
use super::chain::NodeMode;
use super::clipboard::{ClipRequest, Clipboard, Via};
use super::config::TuiConfig;
use super::format::{self, ErrorView};
use super::keymap::{Action, Caps, Keymap};
use super::message::{Command, Connection, Op, OpenRequest, Snapshot, SwitchKind, WorkerEvent};
use super::modal::{Modal, Outcome};
use super::screens::{self, Balances, Ctx, Screen, View};
use super::startup::{Onboarding, Unlock};
use super::theme::{Theme, Tone};
use crate::session::{NewWallet, StoredKeys};

/// What is in the data directory when the TUI starts.
pub enum StartupWallet {
    /// Nothing yet: run onboarding.
    Missing,
    /// A wallet exists; keys are stored like this.
    Existing(StoredKeys),
    /// Demo mode: create a throwaway wallet automatically.
    Demo,
    /// No wallet was named and the default one does not exist: let the
    /// user pick a wallet or name a new one.
    Pick,
}

/// Which part of the interface is showing.
pub enum Phase {
    /// Waiting for the chain backend.
    Connecting,
    /// Picking a wallet, or naming a new one, before anything is open.
    Pick(Modal),
    /// Asking for the password.
    Unlock(Unlock),
    /// Create / restore wizard (boxed: it is much larger than the others).
    Onboarding(Box<Onboarding>),
    /// Waiting for the wallet to open.
    Opening,
    /// The main interface.
    Ready,
    /// Something unrecoverable; only quitting is possible.
    Fatal(ErrorView),
}

/// Background sync status.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyncState {
    /// Not syncing.
    Idle {
        /// When the last sync finished.
        last: Option<Instant>,
        /// Whether the last sync failed.
        failed: bool,
    },
    /// Syncing now.
    Running {
        /// When it started.
        since: Instant,
        /// User-initiated.
        manual: bool,
        /// Blocks fetched.
        done: u32,
        /// Blocks expected.
        total: u32,
    },
}

/// A notification.
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    /// Color / icon.
    pub tone: Tone,
    /// Message.
    pub text: String,
    /// When it appeared.
    pub created: Instant,
}

/// The whole interface state.
pub struct App {
    /// Current phase.
    pub phase: Phase,
    /// Wallet network.
    pub network: Network,
    /// Demo mode: a throwaway wallet is created automatically.
    pub demo: bool,
    /// Where chain data comes from.
    pub node: NodeMode,
    /// Name of the open wallet, for the header.
    pub wallet_name: Option<String>,
    /// Whether other named wallets can be opened (`w`).
    pub can_switch: bool,
    /// Polar mode: the local node can be used instead while Polar is down.
    pub can_fallback: bool,
    /// Node reachability; `None` until the worker's first report.
    pub connection: Option<Connection>,
    /// Payments signed while offline and waiting to be broadcast.
    pub outbox: usize,
    /// Settings.
    pub config: TuiConfig,
    /// Colors.
    pub theme: Theme,
    /// Global keys.
    pub keymap: Keymap,
    /// Backend description.
    pub chain_label: String,
    /// Main-interface tabs.
    pub screens: Vec<Box<dyn Screen>>,
    /// Active tab.
    pub active: usize,
    /// Latest wallet state.
    pub snapshot: Option<Snapshot>,
    /// True after the first completed sync.
    pub synced: bool,
    /// Sync status.
    pub sync: SyncState,
    /// Notifications, oldest first.
    pub toasts: Vec<Toast>,
    /// Open dialog.
    pub modal: Option<Modal>,
    /// Set when the user quits.
    pub should_quit: bool,
    /// Recovery words waiting for "copy? y/n".
    pub confirm_copy: Option<Zeroizing<String>>,
    /// Clipboard work for the event loop.
    clip_requests: Vec<ClipRequest>,
    /// Recovery words on the clipboard, and when they were copied.
    copied_secret: Option<(Instant, Zeroizing<String>)>,
    startup: Option<StartupWallet>,
    /// True from a failed sync until the next successful one, so background
    /// retries do not repeat the same notification.
    sync_failing: bool,
    tweens: [Tween; 3],
    flashes: HashMap<Txid, Instant>,
    loading_since: Instant,
    ready_at: Option<Instant>,
    /// Last time the node's state was reported, for reconnect pacing.
    last_contact: Option<Instant>,
}

impl App {
    /// A new app waiting for the chain backend.
    pub fn new(
        network: Network,
        startup: StartupWallet,
        config: TuiConfig,
        theme: Theme,
        now: Instant,
    ) -> Self {
        let demo = matches!(startup, StartupWallet::Demo);
        Self {
            phase: Phase::Connecting,
            network,
            demo,
            node: if demo {
                NodeMode::Local
            } else {
                NodeMode::External
            },
            wallet_name: None,
            can_switch: false,
            can_fallback: false,
            connection: None,
            outbox: 0,
            screens: screens::all(network, &config),
            config,
            theme,
            keymap: Keymap::default(),
            chain_label: String::new(),
            active: 0,
            snapshot: None,
            synced: false,
            sync: SyncState::Idle {
                last: None,
                failed: false,
            },
            toasts: Vec::new(),
            modal: None,
            should_quit: false,
            confirm_copy: None,
            clip_requests: Vec::new(),
            copied_secret: None,
            startup: Some(startup),
            sync_failing: false,
            tweens: [Tween::at(0, now); 3],
            flashes: HashMap::new(),
            loading_since: now,
            ready_at: None,
            last_contact: None,
        }
    }

    /// True while the node can be reached.
    pub fn online(&self) -> bool {
        matches!(self.connection, Some(Connection::Online { .. }))
    }

    /// What the conditional keys may do right now.
    pub fn caps(&self) -> Caps {
        Caps {
            mining: matches!(
                self.connection,
                Some(Connection::Online { can_mine: true, .. })
            ),
            outbox: self.online() && self.outbox > 0,
            fallback: self.can_fallback
                && matches!(self.connection, Some(Connection::Offline { .. })),
        }
    }

    /// When the current loading animation started.
    pub fn loading_since(&self) -> Instant {
        self.loading_since
    }

    /// Notify the user.
    pub fn notify(&mut self, tone: Tone, text: impl Into<String>, now: Instant) {
        self.toasts.push(Toast {
            tone,
            text: text.into(),
            created: now,
        });
    }

    /// Footer hints: the active screen's keys, then the global ones.
    pub fn hints(&self) -> Vec<(String, String)> {
        let mut hints = self.screens[self.active].hints();
        if !self.screens[self.active].captures_input() {
            hints.extend(self.keymap.footer(self.caps()));
        }
        hints
    }

    /// Everything a screen needs to draw.
    pub fn view(&self, now: Instant) -> View<'_> {
        View {
            snapshot: self.snapshot.as_ref(),
            synced: self.synced,
            theme: &self.theme,
            config: &self.config,
            now,
            loading_since: self.loading_since,
            balances: Balances {
                confirmed: self.tweens[0].value(now),
                unconfirmed: self.tweens[1].value(now),
                immature: self.tweens[2].value(now),
            },
            flashes: &self.flashes,
            chain_label: &self.chain_label,
            online: self.online(),
            offline_reason: match &self.connection {
                Some(Connection::Offline { reason, .. }) => Some(reason),
                _ => None,
            },
            can_switch: self.can_switch,
            outbox: self.outbox,
        }
    }

    // === Input

    /// Handle a key press.
    pub fn on_key(&mut self, key: KeyEvent, now: Instant) -> Vec<Command> {
        if Keymap::is_force_quit(key) {
            self.should_quit = true;
            return Vec::new();
        }
        if let Some(words) = self.confirm_copy.take() {
            match key.code {
                KeyCode::Char('y' | 'Y') => self.clip_requests.push(ClipRequest::Copy {
                    text: words,
                    what: "recovery words",
                    secret: true,
                }),
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {}
                _ => self.confirm_copy = Some(words),
            }
            return Vec::new();
        }
        if is_plain(key, 'c')
            && let Some(words) = self.words_on_screen()
        {
            self.confirm_copy = Some(Zeroizing::new(words.to_string()));
            return Vec::new();
        }
        let fallback = self.caps().fallback;
        match &mut self.phase {
            Phase::Connecting | Phase::Opening | Phase::Fatal(_) => {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    self.should_quit = true;
                }
                Vec::new()
            }
            Phase::Pick(Modal::Wallets { naming: None, .. })
                if matches!(key.code, KeyCode::Char('l' | 'L')) && fallback =>
            {
                self.use_local(now)
            }
            Phase::Pick(modal) => {
                let mut commands = Vec::new();
                // Closing the picker without choosing leaves nothing to show.
                if let Outcome::Close = modal.on_key(key, &mut commands, &self.config)
                    && commands.is_empty()
                {
                    self.should_quit = true;
                }
                commands
            }
            Phase::Unlock(unlock) => unlock.on_key(key).into_iter().collect(),
            Phase::Onboarding(onboarding) => onboarding.on_key(key).into_iter().collect(),
            Phase::Ready => self.ready_key(key, now),
        }
    }

    fn ready_key(&mut self, key: KeyEvent, now: Instant) -> Vec<Command> {
        if let Some(mut modal) = self.modal.take() {
            let mut commands = Vec::new();
            match modal.on_key(key, &mut commands, &self.config) {
                Outcome::Keep => self.modal = Some(modal),
                Outcome::Close => {}
                Outcome::Open(next) => self.modal = Some(next),
            }
            return commands;
        }
        if self.screens[self.active].captures_input() {
            return self.with_screen(self.active, now, |s, cx| s.on_key(key, cx));
        }
        match self.keymap.action(key, self.caps()) {
            Some(Action::Quit) => {
                self.should_quit = true;
                Vec::new()
            }
            Some(Action::NextScreen) => self.show((self.active + 1) % self.screens.len(), now),
            Some(Action::PrevScreen) => self.show(
                (self.active + self.screens.len() - 1) % self.screens.len(),
                now,
            ),
            Some(Action::Screen(i)) if i < self.screens.len() => self.show(i, now),
            Some(Action::Sync) => {
                if matches!(self.sync, SyncState::Running { .. }) {
                    return Vec::new();
                }
                if !self.online() {
                    return match &self.connection {
                        Some(Connection::Offline {
                            retrying: false, ..
                        }) => {
                            self.notify(Tone::Warning, "No node configured; cannot sync", now);
                            Vec::new()
                        }
                        _ => {
                            self.notify(Tone::Info, "Offline; trying to reach the node…", now);
                            self.last_contact = Some(now);
                            vec![Command::Reconnect]
                        }
                    };
                }
                vec![Command::Sync { manual: true }]
            }
            Some(Action::Help) => {
                let mut keys = self.screens[self.active].hints();
                keys.extend(self.keymap.all(self.caps()));
                self.modal = Some(Modal::Help { keys });
                Vec::new()
            }
            Some(Action::Mine) => vec![Command::Mine { blocks: 1 }],
            Some(Action::Faucet) => {
                self.notify(Tone::Info, "Mining 101 blocks to this wallet…", now);
                vec![Command::Faucet]
            }
            Some(Action::Wallets) => {
                if !self.can_switch {
                    self.notify(
                        Tone::Warning,
                        "Switching needs named wallets (not --datadir or --demo)",
                        now,
                    );
                    return Vec::new();
                }
                self.modal = Some(Modal::Wallets {
                    list: None,
                    current: self.wallet_name.clone(),
                    selected: 0,
                    naming: None,
                });
                vec![Command::ListWallets]
            }
            Some(Action::UseLocal) => self.use_local(now),
            Some(Action::SendSaved) => {
                let s = if self.outbox == 1 { "" } else { "s" };
                self.notify(
                    Tone::Info,
                    format!("Broadcasting {} saved payment{s}…", self.outbox),
                    now,
                );
                vec![Command::BroadcastSaved]
            }
            _ => self.with_screen(self.active, now, |s, cx| s.on_key(key, cx)),
        }
    }

    fn show(&mut self, index: usize, now: Instant) -> Vec<Command> {
        self.active = index;
        self.with_screen(index, now, |s, cx| s.on_show(cx))
    }

    /// Run `f` on one screen with a context, collecting its commands and
    /// notifications.
    fn with_screen(
        &mut self,
        index: usize,
        now: Instant,
        f: impl FnOnce(&mut dyn Screen, &mut Ctx),
    ) -> Vec<Command> {
        let mut commands = Vec::new();
        let mut notices = Vec::new();
        let mut copies = Vec::new();
        let mut cx = Ctx {
            snapshot: self.snapshot.as_ref(),
            config: &self.config,
            online: self.online(),
            commands: &mut commands,
            notices: &mut notices,
            modal: &mut self.modal,
            copies: &mut copies,
            now,
        };
        f(self.screens[index].as_mut(), &mut cx);
        self.clip_requests.extend(copies);
        for (tone, text) in notices {
            self.notify(tone, text, now);
        }
        commands
    }

    // === Clipboard

    /// Recovery words currently on screen, which `c` offers to copy.
    fn words_on_screen(&self) -> Option<&str> {
        match &self.phase {
            Phase::Onboarding(onboarding) => onboarding.shown_words(),
            // The demo welcome dialog is shown as early as `Opening`, since
            // demo mode skips onboarding.
            _ => match &self.modal {
                Some(Modal::Welcome { mnemonic }) => Some(mnemonic.as_str()),
                _ => None,
            },
        }
    }

    /// Quitting: take copied recovery words off the clipboard now rather
    /// than leave them for a clipboard manager to keep.
    pub fn on_quit(&mut self) {
        if let Some((_, text)) = self.copied_secret.take() {
            self.clip_requests.push(ClipRequest::Clear { text });
        }
    }

    /// Run queued clipboard work. Called by the event loop between frames.
    pub fn run_clipboard(&mut self, clipboard: &mut dyn Clipboard, now: Instant) {
        for request in std::mem::take(&mut self.clip_requests) {
            match request {
                ClipRequest::Copy { text, what, secret } => match clipboard.set(&text) {
                    Ok(via) => {
                        let mut message = format!("Copied the {what}");
                        if secret {
                            message.push_str(&format!(
                                "; the clipboard is cleared in {} s",
                                self.config.clipboard_clear_secs
                            ));
                            self.copied_secret = Some((now, text));
                        }
                        if via == Via::Terminal {
                            message.push_str(
                                " (sent through the terminal; if pasting gives nothing, \
                                 your terminal does not support it)",
                            );
                        }
                        self.notify(Tone::Success, message, now);
                    }
                    Err(e) => {
                        let hint = if secret {
                            "; write them down instead"
                        } else {
                            ""
                        };
                        self.notify(Tone::Error, format!("Could not copy: {e}{hint}"), now);
                    }
                },
                ClipRequest::Clear { text } => clipboard.clear_if(&text),
            }
        }
    }

    // === Worker events

    /// Handle a worker event.
    pub fn on_event(&mut self, event: WorkerEvent, now: Instant) -> Vec<Command> {
        let mut commands = Vec::new();
        match &event {
            WorkerEvent::Connection(connection) => {
                commands.extend(self.connection_changed(connection.clone(), now));
            }
            WorkerEvent::Opened(snapshot) => {
                self.phase = Phase::Ready;
                self.ready_at = Some(now);
                self.loading_since = now;
                if !snapshot.can_sign {
                    self.notify(Tone::Warning, "Opened watch-only: sending is disabled", now);
                }
                self.apply(snapshot.clone(), now, false);
                if self.online() {
                    commands.push(Command::Sync { manual: false });
                } else {
                    // Nothing newer is coming; show the stored data as is.
                    self.synced = true;
                }
                commands.extend(self.show(self.active, now));
            }
            WorkerEvent::Saved { path, snapshot, .. } => {
                self.apply(snapshot.clone(), now, false);
                let file = path
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.notify(
                    Tone::Warning,
                    format!(
                        "Offline: payment signed and saved to outbox/{file}; \
                         it is sent automatically when the node is back"
                    ),
                    now,
                );
            }
            WorkerEvent::Outbox { count } => self.outbox = *count,
            WorkerEvent::Wallets {
                list: wallets,
                current,
            } => {
                let picker = match &mut self.phase {
                    Phase::Pick(modal) => Some(modal),
                    _ => self.modal.as_mut(),
                };
                if let Some(Modal::Wallets {
                    list,
                    current: open,
                    selected,
                    naming,
                }) = picker
                {
                    // Nothing to pick yet: go straight to naming one.
                    if wallets.is_empty() && naming.is_none() {
                        *naming = Some(Modal::name_field(Vec::new()));
                    }
                    *selected = current
                        .as_ref()
                        .and_then(|c| wallets.iter().position(|w| &w.name == c))
                        .unwrap_or(0);
                    *list = Some(wallets.clone());
                    *open = current.clone();
                }
            }
            WorkerEvent::Switched { name, kind } => {
                self.switched(name.clone(), *kind, now);
            }
            WorkerEvent::SavedBroadcast {
                sent,
                rejected,
                snapshot,
            } => {
                self.apply(snapshot.clone(), now, false);
                if !sent.is_empty() {
                    let s = if sent.len() == 1 { "" } else { "s" };
                    self.notify(
                        Tone::Success,
                        format!("Sent {} saved payment{s}", sent.len()),
                        now,
                    );
                }
                for (file, reason) in rejected {
                    self.notify(Tone::Error, format!("{file} refused: {reason}"), now);
                }
            }
            WorkerEvent::SyncStarted { manual } => {
                self.sync = SyncState::Running {
                    since: now,
                    manual: *manual,
                    done: 0,
                    total: 0,
                };
            }
            WorkerEvent::SyncProgress { done, total } => {
                if let SyncState::Running { since, manual, .. } = self.sync {
                    self.sync = SyncState::Running {
                        since,
                        manual,
                        done: *done,
                        total: *total,
                    };
                }
            }
            WorkerEvent::Synced {
                report,
                snapshot,
                manual,
            } => {
                let first = !self.synced;
                self.synced = true;
                if self.sync_failing {
                    self.sync_failing = false;
                    self.notify(Tone::Success, "Connection to the node restored", now);
                }
                self.sync = SyncState::Idle {
                    last: Some(now),
                    failed: false,
                };
                self.apply(snapshot.clone(), now, !first);
                if *manual {
                    self.notify(
                        Tone::Info,
                        format!(
                            "Synced to block {} ({} new)",
                            report.to.height, report.blocks_applied
                        ),
                        now,
                    );
                }
                if report.reorg_depth > 0 {
                    self.notify(
                        Tone::Warning,
                        format!(
                            "Chain reorganised: up to {} blocks replaced",
                            report.reorg_depth
                        ),
                        now,
                    );
                }
            }
            WorkerEvent::Address(info) => {
                if let Some(s) = &mut self.snapshot {
                    s.receive = info.clone();
                }
            }
            WorkerEvent::Sent { snapshot, .. } | WorkerEvent::Bumped { snapshot, .. } => {
                self.apply(snapshot.clone(), now, false);
            }
            WorkerEvent::Mined { blocks } => {
                let s = if *blocks == 1 { "" } else { "s" };
                self.notify(Tone::Info, format!("Mined {blocks} block{s}"), now);
            }
            WorkerEvent::Failed { op, error } => self.failed(*op, error, now),
            WorkerEvent::FeeEstimate { .. }
            | WorkerEvent::Preview(_)
            | WorkerEvent::PayeeAddress { .. } => {}
        }
        if matches!(self.phase, Phase::Ready) {
            for i in 0..self.screens.len() {
                commands.extend(self.with_screen(i, now, |s, cx| s.on_event(&event, cx)));
            }
        }
        commands
    }

    /// Another wallet was selected: forget the old one's state and get the
    /// new one open (unlock or set up first if needed).
    fn switched(&mut self, name: String, kind: SwitchKind, now: Instant) {
        self.forget_wallet(now);
        self.phase = match kind {
            SwitchKind::Opening => Phase::Opening,
            SwitchKind::Locked => Phase::Unlock(Unlock::default()),
            SwitchKind::New => Phase::Onboarding(Box::new(
                Onboarding::new(self.config.mnemonic_length(), self.network)
                    .named(Some(name.clone())),
            )),
        };
        let what = match kind {
            SwitchKind::New => format!("Creating wallet {name}"),
            _ => format!("Switched to {name}"),
        };
        self.notify(Tone::Info, what, now);
        self.wallet_name = Some(name);
    }

    /// Polar is down and the user asked for the local node: its wallets are
    /// a separate set, so start again at the wallet picker.
    fn use_local(&mut self, now: Instant) -> Vec<Command> {
        self.can_fallback = false;
        self.node = NodeMode::Local;
        self.forget_wallet(now);
        self.wallet_name = None;
        self.chain_label.clear();
        self.phase = Phase::Pick(Modal::Wallets {
            list: None,
            current: None,
            selected: 0,
            naming: None,
        });
        self.notify(
            Tone::Info,
            "Using the local node and its wallets (Polar wallets stay in .wallet/polar)",
            now,
        );
        vec![Command::UseLocalNode, Command::ListWallets]
    }

    /// Drop everything shown about the open wallet.
    fn forget_wallet(&mut self, now: Instant) {
        self.snapshot = None;
        self.synced = false;
        self.outbox = 0;
        self.flashes.clear();
        self.tweens = [Tween::at(0, now); 3];
        self.screens = screens::all(self.network, &self.config);
        self.sync = SyncState::Idle {
            last: None,
            failed: false,
        };
        self.sync_failing = false;
        self.loading_since = now;
        self.modal = None;
    }

    /// The node became reachable or unreachable.
    fn connection_changed(&mut self, connection: Connection, now: Instant) -> Vec<Command> {
        let first = self.connection.is_none();
        let was_online = self.online();
        self.last_contact = Some(now);
        let mut commands = Vec::new();
        match &connection {
            Connection::Online { label, .. } => {
                self.chain_label = label.clone();
                if !first && !was_online {
                    self.notify(Tone::Success, "Back online", now);
                    if matches!(self.phase, Phase::Ready) {
                        commands.push(Command::Sync { manual: false });
                    }
                }
            }
            Connection::Offline { reason, retrying } => {
                if first || was_online {
                    let what = if *retrying {
                        "Offline (retrying in the background)"
                    } else {
                        "No node configured"
                    };
                    let tip = if self.can_fallback {
                        " Press L to use the local node instead."
                    } else {
                        ""
                    };
                    self.notify(Tone::Warning, format!("{what}: {reason}.{tip}"), now);
                }
                if was_online && matches!(self.sync, SyncState::Running { .. }) {
                    self.sync = SyncState::Idle {
                        last: Some(now),
                        failed: true,
                    };
                }
            }
        }
        self.connection = Some(connection);
        if first {
            commands.extend(self.start(now));
        }
        commands
    }

    /// The first node report arrived: decide how to get a wallet open.
    /// Opening never needs the node, so this runs online or offline.
    fn start(&mut self, now: Instant) -> Vec<Command> {
        match self.startup.take() {
            Some(StartupWallet::Demo) => {
                let Ok(words) = wallet::generate_mnemonic(self.config.mnemonic_length()) else {
                    self.phase =
                        Phase::Fatal(ErrorView::new("No randomness", "Could not generate keys."));
                    return Vec::new();
                };
                let keys = KeySource::mnemonic(words.as_str());
                self.modal = Some(Modal::Welcome { mnemonic: words });
                self.phase = Phase::Opening;
                vec![Command::Open(OpenRequest::Create(NewWallet {
                    keys,
                    birthday: None,
                    password: None,
                }))]
            }
            Some(StartupWallet::Pick) => {
                self.phase = Phase::Pick(Modal::Wallets {
                    list: None,
                    current: None,
                    selected: 0,
                    naming: None,
                });
                vec![Command::ListWallets]
            }
            Some(StartupWallet::Missing) => {
                self.phase = Phase::Onboarding(Box::new(
                    Onboarding::new(self.config.mnemonic_length(), self.network)
                        .named(self.wallet_name.clone()),
                ));
                Vec::new()
            }
            Some(StartupWallet::Existing(StoredKeys::Encrypted)) => {
                self.phase = Phase::Unlock(Unlock::default());
                Vec::new()
            }
            Some(StartupWallet::Existing(StoredKeys::Plain(keys))) => {
                self.phase = Phase::Opening;
                vec![Command::Open(OpenRequest::Existing { keys: Some(keys) })]
            }
            Some(StartupWallet::Existing(StoredKeys::None)) => {
                self.phase = Phase::Opening;
                vec![Command::Open(OpenRequest::Existing { keys: None })]
            }
            None => {
                self.loading_since = now;
                Vec::new()
            }
        }
    }

    fn failed(&mut self, op: Op, error: &ErrorView, now: Instant) {
        match (&mut self.phase, op) {
            (Phase::Unlock(unlock), Op::Unlock | Op::Open) => unlock.failed(error),
            (Phase::Onboarding(onboarding), Op::Open) => onboarding.failed(error),
            (Phase::Opening, _) => self.phase = Phase::Fatal(error.clone()),
            (Phase::Ready, Op::Sync) => {
                let was_failing = self.sync_failing;
                self.sync_failing = true;
                let manual = matches!(self.sync, SyncState::Running { manual: true, .. });
                self.sync = SyncState::Idle {
                    last: Some(now),
                    failed: true,
                };
                // Do not repeat the same toast on every background retry.
                if manual || !was_failing {
                    self.notify(Tone::Error, format!("{}: {}", error.title, error.hint), now);
                }
            }
            // The send screen shows these in its own banner.
            (Phase::Ready, Op::Preview | Op::Send | Op::FeeEstimate) => {}
            // The header already shows the offline state.
            (Phase::Ready, _) if error.title == "Offline" => {}
            (Phase::Ready, _) => {
                self.notify(Tone::Error, format!("{}: {}", error.title, error.hint), now);
            }
            _ => {}
        }
    }

    /// Show a new snapshot, animating balances and flagging new transactions.
    fn apply(&mut self, snapshot: Snapshot, now: Instant, announce: bool) {
        let known: HashSet<Txid> = self
            .snapshot
            .as_ref()
            .map(|s| s.txs.iter().map(|t| t.txid).collect())
            .unwrap_or_default();
        let fresh: Vec<_> = snapshot
            .txs
            .iter()
            .filter(|t| !known.contains(&t.txid))
            .cloned()
            .collect();
        if announce {
            for tx in &fresh {
                self.flashes.insert(tx.txid, now);
                let net = format::net(tx);
                if net > 0 {
                    self.notify(
                        Tone::Success,
                        format!("Received {}", format::sats(Amount::from_sat(net as u64))),
                        now,
                    );
                }
            }
        }
        let duration = Duration::from_millis(self.config.tween_ms);
        let b = snapshot.balance;
        for (tween, value) in self
            .tweens
            .iter_mut()
            .zip([b.confirmed, b.unconfirmed, b.immature])
        {
            tween.retarget(value.to_sat(), now, duration);
        }
        self.snapshot = Some(snapshot);
    }

    // === Time

    /// Advance timers: expire notifications, reconnect when offline,
    /// auto-sync when online.
    pub fn on_tick(&mut self, now: Instant) -> Vec<Command> {
        if let Some((since, _)) = &self.copied_secret
            && now.saturating_duration_since(*since) >= self.config.clipboard_clear()
            && let Some((_, text)) = self.copied_secret.take()
        {
            self.clip_requests.push(ClipRequest::Clear { text });
            self.notify(Tone::Info, "Recovery words cleared from the clipboard", now);
        }
        let ttl = Duration::from_secs(self.config.toast_secs);
        self.toasts
            .retain(|t| !anim::lifecycle(t.created, ttl, now).expired);
        self.flashes.retain(|_, since| anim::flashing(*since, now));

        if let Some(Connection::Offline { retrying: true, .. }) = self.connection {
            let due = self
                .last_contact
                .is_none_or(|t| now.saturating_duration_since(t) >= self.config.reconnect());
            if due {
                self.last_contact = Some(now);
                return vec![Command::Reconnect];
            }
            return Vec::new();
        }
        if !self.online() {
            return Vec::new();
        }

        let (Phase::Ready, Some(every), SyncState::Idle { last, .. }) =
            (&self.phase, self.config.refresh(), self.sync)
        else {
            return Vec::new();
        };
        let base = last.or(self.ready_at).unwrap_or(now);
        if now.saturating_duration_since(base) >= every {
            // Mark as running so the next tick does not queue a duplicate.
            self.sync = SyncState::Running {
                since: now,
                manual: false,
                done: 0,
                total: 0,
            };
            return vec![Command::Sync { manual: false }];
        }
        Vec::new()
    }
}

/// `key` is the letter `ch` with no modifier held.
fn is_plain(key: KeyEvent, ch: char) -> bool {
    key.code == KeyCode::Char(ch) && key.modifiers.difference(KeyModifiers::SHIFT).is_empty()
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyModifiers;
    use wallet::bitcoin::hashes::Hash;
    use wallet::bitcoin::{BlockHash, FeeRate};
    use wallet::{
        AddressInfo, Balance, BlockId, Keychain, MnemonicLength, SyncReport, TxDetails, TxStatus,
    };

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn snapshot(confirmed: u64, txs: Vec<TxDetails>) -> Snapshot {
        let phrase = wallet::generate_mnemonic(MnemonicLength::Words12).unwrap();
        let mut w = wallet::Wallet::builder(Network::Regtest)
            .keys(KeySource::mnemonic(phrase.as_str()))
            .create()
            .unwrap();
        let receive: AddressInfo = w.new_address().unwrap();
        assert_eq!(receive.keychain, Keychain::External);
        Snapshot {
            network: Network::Regtest,
            can_sign: true,
            tip: BlockId {
                height: 10,
                hash: BlockHash::all_zeros(),
            },
            balance: Balance {
                confirmed: Amount::from_sat(confirmed),
                ..Balance::default()
            },
            utxos: Vec::new(),
            txs,
            receive,
        }
    }

    fn tx(byte: u8, received: u64) -> TxDetails {
        TxDetails {
            txid: Txid::from_byte_array([byte; 32]),
            sent: Amount::ZERO,
            received: Amount::from_sat(received),
            fee: None,
            status: TxStatus::Unconfirmed,
        }
    }

    fn synced(snapshot: Snapshot, manual: bool) -> WorkerEvent {
        WorkerEvent::Synced {
            report: SyncReport {
                from: snapshot.tip,
                to: snapshot.tip,
                blocks_applied: 2,
                reorg_depth: 0,
                mempool_txs: 0,
            },
            snapshot,
            manual,
        }
    }

    fn ready_app(now: Instant) -> App {
        let mut app = App::new(
            Network::Regtest,
            StartupWallet::Existing(StoredKeys::None),
            TuiConfig::default(),
            Theme::default(),
            now,
        );
        app.on_event(
            WorkerEvent::Connection(Connection::Online {
                label: "mock".into(),
                can_mine: false,
            }),
            now,
        );
        app.on_event(WorkerEvent::Opened(snapshot(0, vec![])), now);
        app.on_event(synced(snapshot(0, vec![]), false), now);
        app.toasts.clear();
        app
    }

    #[test]
    fn startup_routes_by_what_is_on_disk() {
        let now = Instant::now();
        let ready = WorkerEvent::Connection(Connection::Online {
            label: "x".into(),
            can_mine: false,
        });

        let mut missing = App::new(
            Network::Regtest,
            StartupWallet::Missing,
            TuiConfig::default(),
            Theme::default(),
            now,
        );
        assert!(missing.on_event(ready.clone(), now).is_empty());
        assert!(matches!(missing.phase, Phase::Onboarding(_)));

        let mut locked = App::new(
            Network::Regtest,
            StartupWallet::Existing(StoredKeys::Encrypted),
            TuiConfig::default(),
            Theme::default(),
            now,
        );
        locked.on_event(ready.clone(), now);
        assert!(matches!(locked.phase, Phase::Unlock(_)));

        let mut demo = App::new(
            Network::Regtest,
            StartupWallet::Demo,
            TuiConfig::default(),
            Theme::default(),
            now,
        );
        let commands = demo.on_event(ready, now);
        assert!(matches!(
            commands.as_slice(),
            [Command::Open(OpenRequest::Create(_))]
        ));
        assert!(
            matches!(demo.modal, Some(Modal::Welcome { .. })),
            "demo shows its fresh words"
        );
    }

    #[test]
    fn opening_triggers_a_first_sync_and_shows_skeletons_until_then() {
        let now = Instant::now();
        let mut app = App::new(
            Network::Regtest,
            StartupWallet::Existing(StoredKeys::None),
            TuiConfig::default(),
            Theme::default(),
            now,
        );
        app.on_event(
            WorkerEvent::Connection(Connection::Online {
                label: "x".into(),
                can_mine: false,
            }),
            now,
        );
        let commands = app.on_event(WorkerEvent::Opened(snapshot(0, vec![])), now);
        assert!(matches!(
            commands.as_slice(),
            [Command::Sync { manual: false }, ..]
        ));
        assert!(!app.synced, "skeletons until the first sync completes");
        app.on_event(synced(snapshot(0, vec![]), false), now);
        assert!(app.synced);
    }

    #[test]
    fn balances_count_up_and_new_payments_are_announced() {
        let t0 = Instant::now();
        let mut app = ready_app(t0);
        app.on_event(synced(snapshot(50_000, vec![tx(1, 50_000)]), false), t0);

        assert_eq!(
            app.view(t0).balances.confirmed,
            0,
            "starts from the old value"
        );
        let mid = app.view(t0 + ms(300)).balances.confirmed;
        assert!(mid > 0 && mid < 50_000, "{mid}");
        assert_eq!(app.view(t0 + ms(2_000)).balances.confirmed, 50_000);

        assert!(
            app.toasts
                .iter()
                .any(|t| t.tone == Tone::Success && t.text.contains("50,000"))
        );
        assert!(
            app.view(t0)
                .flashes
                .contains_key(&Txid::from_byte_array([1; 32]))
        );
    }

    #[test]
    fn global_keys_switch_screens_sync_and_quit() {
        let now = Instant::now();
        let mut app = ready_app(now);
        app.on_key(key(KeyCode::Char('4')), now);
        assert_eq!(app.screens[app.active].title(), "History");
        app.on_key(key(KeyCode::Tab), now);
        assert_eq!(app.screens[app.active].title(), "UTXOs");
        let sync = app.on_key(key(KeyCode::Char('r')), now);
        assert!(matches!(sync.as_slice(), [Command::Sync { manual: true }]));
        app.on_key(key(KeyCode::Char('?')), now);
        assert!(matches!(app.modal, Some(Modal::Help { .. })));
        app.on_key(key(KeyCode::Esc), now);
        assert!(app.modal.is_none());
        app.on_key(key(KeyCode::Char('q')), now);
        assert!(app.should_quit);
    }

    #[test]
    fn typing_in_a_form_does_not_trigger_global_keys() {
        let now = Instant::now();
        let mut app = ready_app(now);
        let commands = app.on_key(key(KeyCode::Char('3')), now);
        assert!(
            matches!(commands.as_slice(), [Command::EstimateFee { .. }]),
            "send screen asks for a fee"
        );
        app.on_key(key(KeyCode::Enter), now);
        for c in "qr4".chars() {
            assert!(app.on_key(key(KeyCode::Char(c)), now).is_empty());
        }
        assert!(!app.should_quit, "q was typed, not quit");
        assert_eq!(app.screens[app.active].title(), "Send");
        app.on_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            now,
        );
        assert!(app.should_quit, "ctrl+c always quits");
    }

    #[test]
    fn auto_sync_runs_on_the_configured_interval() {
        let t0 = Instant::now();
        let mut app = ready_app(t0);
        assert!(app.on_tick(t0 + ms(5_000)).is_empty());
        let due = app.on_tick(t0 + ms(10_000));
        assert!(matches!(due.as_slice(), [Command::Sync { manual: false }]));
        assert!(
            app.on_tick(t0 + ms(10_100)).is_empty(),
            "no duplicate while running"
        );

        let mut off = ready_app(t0);
        off.config.refresh_secs = 0;
        assert!(off.on_tick(t0 + ms(60_000)).is_empty());
    }

    #[test]
    fn repeated_background_sync_failures_toast_once() {
        let now = Instant::now();
        let mut app = ready_app(now);
        let failed = || WorkerEvent::Failed {
            op: Op::Sync,
            error: ErrorView::new("Node unreachable", "Retrying."),
        };
        app.on_event(WorkerEvent::SyncStarted { manual: false }, now);
        app.on_event(failed(), now);
        app.on_event(WorkerEvent::SyncStarted { manual: false }, now);
        app.on_event(failed(), now);
        assert_eq!(app.toasts.len(), 1);
        assert!(matches!(app.sync, SyncState::Idle { failed: true, .. }));

        // Recovery is announced once, and a later failure toasts again.
        app.on_event(synced(snapshot(0, vec![]), false), now);
        assert!(app.toasts.iter().any(|t| t.text.contains("restored")));
        app.on_event(WorkerEvent::SyncStarted { manual: false }, now);
        app.on_event(failed(), now);
        assert_eq!(
            app.toasts.iter().filter(|t| t.tone == Tone::Error).count(),
            2
        );
    }

    #[test]
    fn toasts_expire_and_mining_keys_need_a_mining_node() {
        let t0 = Instant::now();
        let mut app = ready_app(t0);
        app.notify(Tone::Info, "hello", t0);
        app.on_tick(t0 + ms(1_000));
        assert_eq!(app.toasts.len(), 1);
        app.on_tick(t0 + ms(5_000));
        assert!(app.toasts.is_empty());

        assert!(app.on_key(key(KeyCode::Char('m')), t0).is_empty());
        app.connection = Some(Connection::Online {
            label: "local".into(),
            can_mine: true,
        });
        assert!(matches!(
            app.on_key(key(KeyCode::Char('m')), t0).as_slice(),
            [Command::Mine { blocks: 1 }]
        ));
    }

    #[test]
    fn open_failure_while_opening_is_fatal_but_unlock_failure_is_retryable() {
        let now = Instant::now();
        let error = ErrorView::new("Could not open the wallet", "corrupt");
        let mut app = App::new(
            Network::Regtest,
            StartupWallet::Existing(StoredKeys::None),
            TuiConfig::default(),
            Theme::default(),
            now,
        );
        app.on_event(
            WorkerEvent::Connection(Connection::Online {
                label: "x".into(),
                can_mine: false,
            }),
            now,
        );
        app.on_event(
            WorkerEvent::Failed {
                op: Op::Open,
                error: error.clone(),
            },
            now,
        );
        assert!(matches!(app.phase, Phase::Fatal(_)));

        let mut locked = App::new(
            Network::Regtest,
            StartupWallet::Existing(StoredKeys::Encrypted),
            TuiConfig::default(),
            Theme::default(),
            now,
        );
        locked.on_event(
            WorkerEvent::Connection(Connection::Online {
                label: "x".into(),
                can_mine: false,
            }),
            now,
        );
        locked.on_event(
            WorkerEvent::Failed {
                op: Op::Unlock,
                error,
            },
            now,
        );
        assert!(matches!(locked.phase, Phase::Unlock(_)), "can try again");
    }

    #[test]
    fn offline_at_start_still_opens_the_wallet_and_shows_stored_data() {
        let now = Instant::now();
        let mut app = App::new(
            Network::Regtest,
            StartupWallet::Existing(StoredKeys::None),
            TuiConfig::default(),
            Theme::default(),
            now,
        );
        let offline = WorkerEvent::Connection(Connection::Offline {
            reason: "connection refused".into(),
            retrying: true,
        });
        let commands = app.on_event(offline, now);
        assert!(
            matches!(
                commands.as_slice(),
                [Command::Open(OpenRequest::Existing { .. })]
            ),
            "opening does not wait for the node"
        );
        assert!(app.toasts.iter().any(|t| t.text.contains("Offline")));

        let commands = app.on_event(WorkerEvent::Opened(snapshot(70_000, vec![])), now);
        assert!(
            !commands.iter().any(|c| matches!(c, Command::Sync { .. })),
            "no sync while offline"
        );
        assert!(app.synced, "stored data is shown instead of skeletons");
        assert!(!app.online());
    }

    #[test]
    fn offline_retries_on_an_interval_and_resyncs_when_back() {
        let t0 = Instant::now();
        let mut app = ready_app(t0);
        app.on_event(
            WorkerEvent::Connection(Connection::Offline {
                reason: "refused".into(),
                retrying: true,
            }),
            t0,
        );
        assert!(app.on_tick(t0 + ms(1_000)).is_empty());
        let retry = app.on_tick(t0 + ms(5_000));
        assert!(matches!(retry.as_slice(), [Command::Reconnect]));
        assert!(app.on_tick(t0 + ms(5_100)).is_empty(), "paced");
        assert!(
            app.on_tick(t0 + ms(60_000))
                .iter()
                .all(|c| !matches!(c, Command::Sync { .. })),
            "no auto-sync offline"
        );

        // 'r' while offline retries immediately instead of syncing.
        let r = app.on_key(key(KeyCode::Char('r')), t0);
        assert!(matches!(r.as_slice(), [Command::Reconnect]));

        let back = app.on_event(
            WorkerEvent::Connection(Connection::Online {
                label: "node".into(),
                can_mine: false,
            }),
            t0,
        );
        assert!(matches!(
            back.as_slice(),
            [Command::Sync { manual: false }, ..]
        ));
        assert!(app.toasts.iter().any(|t| t.text == "Back online"));
    }

    #[test]
    fn no_node_mode_never_retries() {
        let t0 = Instant::now();
        let mut app = ready_app(t0);
        app.on_event(
            WorkerEvent::Connection(Connection::Offline {
                reason: "no node configured".into(),
                retrying: false,
            }),
            t0,
        );
        assert!(app.on_tick(t0 + ms(600_000)).is_empty());
        assert!(app.on_key(key(KeyCode::Char('r')), t0).is_empty());
    }

    #[test]
    fn outbox_key_appears_only_online_with_saved_payments() {
        let t0 = Instant::now();
        let mut app = ready_app(t0);
        assert!(app.on_key(key(KeyCode::Char('o')), t0).is_empty());
        app.on_event(WorkerEvent::Outbox { count: 2 }, t0);
        let send = app.on_key(key(KeyCode::Char('o')), t0);
        assert!(matches!(send.as_slice(), [Command::BroadcastSaved]));
        assert!(app.hints().iter().any(|(k, _)| k == "o"));

        app.on_event(
            WorkerEvent::Connection(Connection::Offline {
                reason: "x".into(),
                retrying: true,
            }),
            t0,
        );
        assert!(
            app.on_key(key(KeyCode::Char('o')), t0).is_empty(),
            "needs the node"
        );
    }

    #[test]
    fn mining_keys_need_a_managed_node() {
        let t0 = Instant::now();
        let mut app = ready_app(t0);
        assert!(app.on_key(key(KeyCode::Char('f')), t0).is_empty());
        app.on_event(
            WorkerEvent::Connection(Connection::Online {
                label: "local".into(),
                can_mine: true,
            }),
            t0,
        );
        assert!(matches!(
            app.on_key(key(KeyCode::Char('f')), t0).as_slice(),
            [Command::Faucet]
        ));
    }

    #[test]
    fn wallet_switcher_lists_switches_and_routes_by_kind() {
        let now = Instant::now();
        let mut app = ready_app(now);
        app.wallet_name = Some("alice".into());

        // Without named wallets, `w` explains instead of opening.
        assert!(app.on_key(key(KeyCode::Char('w')), now).is_empty());
        assert!(app.modal.is_none());

        app.can_switch = true;
        let asked = app.on_key(key(KeyCode::Char('w')), now);
        assert!(matches!(asked.as_slice(), [Command::ListWallets]));
        let entry = |name: &str| crate::wallets::Entry {
            name: name.into(),
            encrypted: false,
            last_balance_sat: None,
            is_default: false,
        };
        app.on_event(
            WorkerEvent::Wallets {
                list: vec![entry("alice"), entry("bob")],
                current: Some("alice".into()),
            },
            now,
        );
        app.on_key(key(KeyCode::Down), now);
        let switch = app.on_key(key(KeyCode::Enter), now);
        assert!(matches!(switch.as_slice(), [Command::SwitchWallet { name }] if name == "bob"));
        assert!(app.modal.is_none());

        app.on_event(
            WorkerEvent::Switched {
                name: "bob".into(),
                kind: SwitchKind::Opening,
            },
            now,
        );
        assert!(matches!(app.phase, Phase::Opening));
        assert_eq!(app.wallet_name.as_deref(), Some("bob"));
        assert!(app.snapshot.is_none(), "alice's data is gone");

        app.on_event(
            WorkerEvent::Switched {
                name: "carol".into(),
                kind: SwitchKind::Locked,
            },
            now,
        );
        assert!(matches!(app.phase, Phase::Unlock(_)));
        app.on_event(
            WorkerEvent::Switched {
                name: "dave".into(),
                kind: SwitchKind::New,
            },
            now,
        );
        assert!(matches!(app.phase, Phase::Onboarding(_)));
    }

    #[test]
    fn new_wallet_names_are_validated_in_the_switcher() {
        let now = Instant::now();
        let mut app = ready_app(now);
        app.can_switch = true;
        app.on_key(key(KeyCode::Char('w')), now);
        app.on_event(
            WorkerEvent::Wallets {
                list: vec![crate::wallets::Entry {
                    name: "alice".into(),
                    encrypted: false,
                    last_balance_sat: Some(1),
                    is_default: true,
                }],
                current: Some("alice".into()),
            },
            now,
        );
        app.on_key(key(KeyCode::Char('n')), now);
        for c in "alice".chars() {
            app.on_key(key(KeyCode::Char(c)), now);
        }
        assert!(app.on_key(key(KeyCode::Enter), now).is_empty(), "taken");
        for _ in 0..5 {
            app.on_key(key(KeyCode::Backspace), now);
        }
        for c in "bob".chars() {
            app.on_key(key(KeyCode::Char(c)), now);
        }
        let create = app.on_key(key(KeyCode::Enter), now);
        assert!(matches!(create.as_slice(), [Command::SwitchWallet { name }] if name == "bob"));
    }

    #[test]
    fn history_bump_flow_goes_through_dialogs() {
        let now = Instant::now();
        let mut app = ready_app(now);
        app.on_event(synced(snapshot(10_000, vec![tx(9, 10_000)]), false), now);
        app.on_key(key(KeyCode::Char('4')), now);
        app.on_key(key(KeyCode::Enter), now);
        assert!(matches!(app.modal, Some(Modal::TxDetail { .. })));
        app.on_key(key(KeyCode::Char('b')), now);
        assert!(matches!(app.modal, Some(Modal::BumpFee { .. })));
        app.on_key(key(KeyCode::Char('7')), now);
        let commands = app.on_key(key(KeyCode::Enter), now);
        assert!(matches!(
            commands.as_slice(),
            [Command::BumpFee { fee_rate, .. }] if *fee_rate == FeeRate::from_sat_per_vb_u32(7)
        ));
        assert!(app.modal.is_none());
    }

    #[test]
    fn copying_recovery_words_asks_first_then_clears_after_the_timeout() {
        use super::super::clipboard::tests::FakeClipboard;

        let now = Instant::now();
        let mut app = App::new(
            Network::Regtest,
            StartupWallet::Demo,
            TuiConfig::default(),
            Theme::default(),
            now,
        );
        app.on_event(
            WorkerEvent::Connection(Connection::Online {
                label: "demo".into(),
                can_mine: true,
            }),
            now,
        );
        assert!(matches!(app.modal, Some(Modal::Welcome { .. })));

        // `c` asks for confirmation; anything but y/n/esc leaves it open.
        app.on_key(key(KeyCode::Char('c')), now);
        assert!(app.confirm_copy.is_some());
        app.on_key(key(KeyCode::Char('z')), now);
        assert!(app.confirm_copy.is_some(), "not a yes or a no");
        app.on_key(key(KeyCode::Char('y')), now);
        assert!(app.confirm_copy.is_none());

        let mut clip = FakeClipboard::default();
        app.run_clipboard(&mut clip, now);
        assert!(clip.text.is_some(), "copied after confirming");
        assert!(
            app.toasts.iter().any(|t| t.text.contains("Copied")),
            "{:?}",
            app.toasts
        );

        // Not cleared before the configured delay.
        let before = now + app.config.clipboard_clear() - Duration::from_millis(1);
        app.on_tick(before);
        app.run_clipboard(&mut clip, before);
        assert!(clip.text.is_some(), "too soon");

        // Cleared once it elapses.
        let after = now + app.config.clipboard_clear() + Duration::from_millis(1);
        app.on_tick(after);
        app.run_clipboard(&mut clip, after);
        assert_eq!(clip.text, None);
        assert_eq!(clip.clears, 1);
    }

    #[test]
    fn declining_the_copy_prompt_copies_nothing() {
        use super::super::clipboard::tests::FakeClipboard;

        let now = Instant::now();
        let mut app = App::new(
            Network::Regtest,
            StartupWallet::Demo,
            TuiConfig::default(),
            Theme::default(),
            now,
        );
        app.on_event(
            WorkerEvent::Connection(Connection::Online {
                label: "demo".into(),
                can_mine: true,
            }),
            now,
        );
        app.on_key(key(KeyCode::Char('c')), now);
        app.on_key(key(KeyCode::Char('n')), now);
        assert!(app.confirm_copy.is_none());
        let mut clip = FakeClipboard::default();
        app.run_clipboard(&mut clip, now);
        assert_eq!(clip.text, None);
    }

    #[test]
    fn quitting_clears_a_copied_secret_right_away() {
        use super::super::clipboard::tests::FakeClipboard;

        let now = Instant::now();
        let mut app = App::new(
            Network::Regtest,
            StartupWallet::Demo,
            TuiConfig::default(),
            Theme::default(),
            now,
        );
        app.on_event(
            WorkerEvent::Connection(Connection::Online {
                label: "demo".into(),
                can_mine: true,
            }),
            now,
        );
        app.on_key(key(KeyCode::Char('c')), now);
        app.on_key(key(KeyCode::Char('y')), now);
        let mut clip = FakeClipboard::default();
        app.run_clipboard(&mut clip, now);
        assert!(clip.text.is_some());

        app.on_quit();
        app.run_clipboard(&mut clip, now);
        assert_eq!(clip.text, None, "cleared on quit, not left for a minute");
    }
}
