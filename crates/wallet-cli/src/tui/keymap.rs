//! Global key bindings, as data.
//!
//! One table drives key handling, the footer and the help screen, so a
//! binding is added or changed in exactly one place. Screen-specific keys
//! live with their screen (see `Screen::hints`).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Things the user can do from anywhere in the main interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Leave the TUI.
    Quit,
    /// Next screen.
    NextScreen,
    /// Previous screen.
    PrevScreen,
    /// Jump to screen `n` (0-based).
    Screen(usize),
    /// Sync now (or retry the node when offline).
    Sync,
    /// Show all keys.
    Help,
    /// Managed node: mine one block.
    Mine,
    /// Managed node: mine 101 blocks to this wallet.
    Faucet,
    /// Broadcast payments saved while offline.
    SendSaved,
    /// Open the wallet switcher.
    Wallets,
}

/// When a binding is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Always.
    Nothing,
    /// Only with a managed regtest node that is online.
    Mining,
    /// Only while online with payments waiting in the outbox.
    Outbox,
}

/// What is currently possible, used to filter bindings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Caps {
    /// Blocks can be mined.
    pub mining: bool,
    /// Saved payments can be broadcast.
    pub outbox: bool,
}

impl Caps {
    fn allows(self, need: Need) -> bool {
        match need {
            Need::Nothing => true,
            Need::Mining => self.mining,
            Need::Outbox => self.outbox,
        }
    }
}

/// One key combination and what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// The key.
    pub code: KeyCode,
    /// Required modifiers.
    pub modifiers: KeyModifiers,
    /// What it does.
    pub action: Action,
    /// How the key is written in hints, e.g. `"r"`.
    pub label: &'static str,
    /// What it does, in a few words.
    pub description: &'static str,
    /// When it is available.
    pub needs: Need,
    /// Shown in the footer (others only in help).
    pub in_footer: bool,
}

const fn bind(
    code: KeyCode,
    action: Action,
    label: &'static str,
    description: &'static str,
) -> Binding {
    Binding {
        code,
        modifiers: KeyModifiers::NONE,
        action,
        label,
        description,
        needs: Need::Nothing,
        in_footer: false,
    }
}

/// The active bindings.
#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        let footer = |b: Binding| Binding {
            in_footer: true,
            ..b
        };
        let needs = |need: Need, b: Binding| Binding {
            needs: need,
            in_footer: true,
            ..b
        };
        let mut bindings = vec![
            footer(bind(KeyCode::Char('q'), Action::Quit, "q", "quit")),
            Binding {
                modifiers: KeyModifiers::CONTROL,
                ..bind(KeyCode::Char('c'), Action::Quit, "ctrl+c", "quit")
            },
            footer(bind(KeyCode::Tab, Action::NextScreen, "tab", "next")),
            bind(
                KeyCode::BackTab,
                Action::PrevScreen,
                "shift+tab",
                "previous",
            ),
            footer(bind(KeyCode::Char('r'), Action::Sync, "r", "sync")),
            footer(bind(KeyCode::Char('w'), Action::Wallets, "w", "wallets")),
            footer(bind(KeyCode::Char('?'), Action::Help, "?", "help")),
            needs(
                Need::Mining,
                bind(KeyCode::Char('m'), Action::Mine, "m", "mine block"),
            ),
            needs(
                Need::Mining,
                bind(KeyCode::Char('f'), Action::Faucet, "f", "faucet"),
            ),
            needs(
                Need::Outbox,
                bind(KeyCode::Char('o'), Action::SendSaved, "o", "send saved"),
            ),
        ];
        for (i, digit) in ['1', '2', '3', '4', '5', '6', '7', '8', '9']
            .into_iter()
            .enumerate()
        {
            bindings.push(bind(
                KeyCode::Char(digit),
                Action::Screen(i),
                "1-9",
                "go to screen",
            ));
        }
        Self { bindings }
    }
}

impl Keymap {
    /// The action for `key`, if it is available with `caps`.
    pub fn action(&self, key: KeyEvent, caps: Caps) -> Option<Action> {
        self.bindings
            .iter()
            .find(|b| {
                b.code == key.code && key.modifiers.contains(b.modifiers) && caps.allows(b.needs)
            })
            .map(|b| b.action)
    }

    /// Ctrl+C quits even while typing.
    pub fn is_force_quit(key: KeyEvent) -> bool {
        key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
    }

    /// `(key, description)` pairs for the footer.
    pub fn footer(&self, caps: Caps) -> Vec<(String, String)> {
        self.bindings
            .iter()
            .filter(|b| b.in_footer && caps.allows(b.needs))
            .map(|b| (b.label.to_string(), b.description.to_string()))
            .collect()
    }

    /// Every available binding once, for the help screen.
    pub fn all(&self, caps: Caps) -> Vec<(String, String)> {
        let mut seen = Vec::<(String, String)>::new();
        for b in self.bindings.iter().filter(|b| caps.allows(b.needs)) {
            let pair = (b.label.to_string(), b.description.to_string());
            if !seen.contains(&pair) {
                seen.push(pair);
            }
        }
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn keys_map_to_actions() {
        let keymap = Keymap::default();
        let caps = Caps::default();
        assert_eq!(
            keymap.action(key(KeyCode::Char('q')), caps),
            Some(Action::Quit)
        );
        assert_eq!(
            keymap.action(key(KeyCode::Char('3')), caps),
            Some(Action::Screen(2))
        );
        assert_eq!(
            keymap.action(key(KeyCode::Tab), caps),
            Some(Action::NextScreen)
        );
        assert_eq!(keymap.action(key(KeyCode::Char('x')), caps), None);
    }

    #[test]
    fn conditional_keys_follow_capabilities() {
        let keymap = Keymap::default();
        let none = Caps::default();
        let mining = Caps {
            mining: true,
            ..none
        };
        let outbox = Caps {
            outbox: true,
            ..none
        };
        assert_eq!(keymap.action(key(KeyCode::Char('m')), none), None);
        assert_eq!(
            keymap.action(key(KeyCode::Char('m')), mining),
            Some(Action::Mine)
        );
        assert!(!keymap.footer(none).iter().any(|(k, _)| k == "f"));
        assert!(keymap.footer(mining).iter().any(|(k, _)| k == "f"));
        assert_eq!(keymap.action(key(KeyCode::Char('o')), none), None);
        assert_eq!(
            keymap.action(key(KeyCode::Char('o')), outbox),
            Some(Action::SendSaved)
        );
    }

    #[test]
    fn ctrl_c_always_quits() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(Keymap::is_force_quit(ctrl_c));
        assert_eq!(
            Keymap::default().action(ctrl_c, Caps::default()),
            Some(Action::Quit)
        );
        assert!(!Keymap::is_force_quit(key(KeyCode::Char('c'))));
    }

    #[test]
    fn help_lists_each_binding_once() {
        let all = Keymap::default().all(Caps {
            mining: true,
            outbox: true,
        });
        let digits = all.iter().filter(|(k, _)| k == "1-9").count();
        assert_eq!(digits, 1);
    }
}
