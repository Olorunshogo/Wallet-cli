//! The main-interface screens.
//!
//! Each screen owns its own UI state and implements [`Screen`]. The app keeps
//! them in the list returned by [`all`]; adding a screen means writing one
//! file and adding one line there. Screens never call the wallet: they read
//! the latest [`Snapshot`] and ask for work by pushing [`Command`]s.

pub mod dashboard;
pub mod history;
pub mod receive;
pub mod send;
pub mod utxos;

use std::cell::Cell;
use std::collections::HashMap;
use std::time::Instant;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use wallet::bitcoin::{Network, Txid};

use super::config::TuiConfig;
use super::message::{Command, Snapshot, WorkerEvent};
use super::modal::Modal;
use super::theme::{Theme, Tone};
use crate::pager::{Page, paginate};

/// What a screen may change in response to input or events.
pub struct Ctx<'a> {
    /// Latest wallet state.
    pub snapshot: Option<&'a Snapshot>,
    /// Settings.
    pub config: &'a TuiConfig,
    /// Whether the node can be reached.
    pub online: bool,
    /// Work for the wallet worker.
    pub commands: &'a mut Vec<Command>,
    /// Notifications to show.
    pub notices: &'a mut Vec<(Tone, String)>,
    /// Open a dialog.
    pub modal: &'a mut Option<Modal>,
    /// Current time.
    pub now: Instant,
}

impl Ctx<'_> {
    /// Queue a command for the worker.
    pub fn send(&mut self, command: Command) {
        self.commands.push(command);
    }

    /// Show a notification.
    pub fn notify(&mut self, tone: Tone, text: impl Into<String>) {
        self.notices.push((tone, text.into()));
    }
}

/// Animated balance values for the dashboard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Balances {
    /// Confirmed sats, mid-animation.
    pub confirmed: u64,
    /// Unconfirmed sats, mid-animation.
    pub unconfirmed: u64,
    /// Immature sats, mid-animation.
    pub immature: u64,
}

/// What a screen needs to draw itself.
pub struct View<'a> {
    /// Latest wallet state.
    pub snapshot: Option<&'a Snapshot>,
    /// True once the first sync has finished; before that numbers may be stale.
    pub synced: bool,
    /// Colors.
    pub theme: &'a Theme,
    /// Settings.
    pub config: &'a TuiConfig,
    /// Current time, for animations.
    pub now: Instant,
    /// When loading started, for skeleton shimmer.
    pub loading_since: Instant,
    /// Balances as currently animated.
    pub balances: Balances,
    /// Recently appeared transactions and when they appeared.
    pub flashes: &'a HashMap<Txid, Instant>,
    /// Backend description.
    pub chain_label: &'a str,
    /// Whether the node can be reached.
    pub online: bool,
    /// Payments waiting in the outbox.
    pub outbox: usize,
}

/// One tab of the main interface.
pub trait Screen {
    /// Tab title.
    fn title(&self) -> &'static str;

    /// Screen-specific `(key, description)` hints for the footer.
    fn hints(&self) -> Vec<(String, String)>;

    /// True while the screen is taking text, so global single-letter keys
    /// (like `q` and `r`) are typed instead of acted on.
    fn captures_input(&self) -> bool {
        false
    }

    /// Called when the tab becomes active.
    fn on_show(&mut self, _cx: &mut Ctx) {}

    /// Handle a key the global keymap did not take.
    fn on_key(&mut self, key: KeyEvent, cx: &mut Ctx);

    /// React to a worker event. Every screen sees every event.
    fn on_event(&mut self, _event: &WorkerEvent, _cx: &mut Ctx) {}

    /// Draw into `area`.
    fn render(&self, f: &mut Frame, area: Rect, view: &View);
}

/// The screens, in tab order.
pub fn all(network: Network, config: &TuiConfig) -> Vec<Box<dyn Screen>> {
    vec![
        Box::new(dashboard::Dashboard),
        Box::new(receive::Receive),
        Box::new(send::Send::new(network, config)),
        Box::new(history::History::default()),
        Box::new(utxos::Utxos::default()),
    ]
}

/// Selection and paging for a list screen.
///
/// The page size is whatever fits on screen, learned during the last render,
/// so a taller terminal shows more per page.
#[derive(Debug)]
pub struct Paged {
    /// Selected item, across all pages.
    pub selected: usize,
    per_page: Cell<usize>,
}

impl Default for Paged {
    fn default() -> Self {
        Self {
            selected: 0,
            per_page: Cell::new(10),
        }
    }
}

impl Paged {
    /// Move the selection. Returns true when the key was a navigation key.
    pub fn on_key(&mut self, code: KeyCode, len: usize) -> bool {
        let page = self.per_page.get() as isize;
        self.selected = match code {
            KeyCode::Up | KeyCode::Char('k') => step(self.selected, -1, len),
            KeyCode::Down | KeyCode::Char('j') => step(self.selected, 1, len),
            KeyCode::Left | KeyCode::PageUp => step(self.selected, -page, len),
            KeyCode::Right | KeyCode::PageDown => step(self.selected, page, len),
            KeyCode::Home => 0,
            KeyCode::End => len.saturating_sub(1),
            _ => return false,
        };
        true
    }

    /// Keep the selection inside a list that may have shrunk.
    pub fn clamp(&mut self, len: usize) {
        self.selected = step(self.selected, 0, len);
    }

    /// The page holding the selection, sized to `rows`, and the selection's
    /// position on it.
    pub fn page<'a, T>(&self, items: &'a [T], rows: usize) -> (Page<'a, T>, usize) {
        let rows = rows.max(1);
        self.per_page.set(rows);
        let page = paginate(items, self.selected / rows + 1, rows);
        (page, self.selected % rows)
    }

    /// Footer hints for paging.
    pub fn hints() -> Vec<(String, String)> {
        vec![("↑↓".into(), "select".into()), ("←→".into(), "page".into())]
    }
}

/// Move a list selection by `delta`, clamped to `len`.
pub fn step(selected: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    selected.saturating_add_signed(delta).min(len - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paging_moves_by_the_rendered_page_size() {
        let items: Vec<u32> = (0..45).collect();
        let mut paged = Paged::default();
        let (page, at) = paged.page(&items, 20);
        assert_eq!((page.page, page.pages, at), (1, 3, 0));

        paged.on_key(KeyCode::PageDown, items.len());
        assert_eq!(paged.selected, 20);
        let (page, at) = paged.page(&items, 20);
        assert_eq!((page.page, page.first, at), (2, 21, 0));

        paged.on_key(KeyCode::End, items.len());
        let (page, at) = paged.page(&items, 20);
        assert_eq!((page.page, page.items.len(), at), (3, 5, 4));

        paged.on_key(KeyCode::Right, items.len());
        assert_eq!(paged.selected, 44, "clamped at the end");
        paged.on_key(KeyCode::Home, items.len());
        assert_eq!(paged.selected, 0);
        assert!(!paged.on_key(KeyCode::Char('x'), items.len()));

        paged.selected = 44;
        paged.clamp(10);
        assert_eq!(paged.selected, 9, "list shrank");
    }

    #[test]
    fn selection_is_clamped() {
        assert_eq!(step(0, -1, 5), 0);
        assert_eq!(step(4, 1, 5), 4);
        assert_eq!(step(2, 1, 5), 3);
        assert_eq!(step(3, 1, 0), 0);
    }
}
