//! Before the main interface: unlocking an encrypted wallet, or creating /
//! restoring one when the data directory is empty.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use wallet::bitcoin::Network;
use wallet::{KeySource, MnemonicLength, generate_mnemonic};
use zeroize::Zeroizing;

use super::format::ErrorView;
use super::message::{Command, OpenRequest};
use super::theme::{Theme, Tone};
use super::validate::{
    AnyText, MnemonicValidator, OptionalHeightValidator, PasswordValidator, Validator,
};
use super::widgets::input::{FIELD_HEIGHT, Field, render_field};
use super::widgets::{self, centered, hint_line};
use crate::session::NewWallet;

/// Shortest password accepted for encryption.
pub const MIN_PASSWORD: usize = 8;

// === Unlock

/// Password prompt for an encrypted wallet.
pub struct Unlock {
    field: Field<PasswordValidator>,
    busy: bool,
}

impl Default for Unlock {
    fn default() -> Self {
        Self {
            field: Field::masked(
                "Password",
                "wallet password",
                PasswordValidator { min_len: 1 },
            ),
            busy: false,
        }
    }
}

impl Unlock {
    /// Enter submits; Esc opens watch-only.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<Command> {
        if self.busy {
            return None;
        }
        match key.code {
            KeyCode::Enter => {
                let password = self.field.submit().ok()?;
                self.busy = true;
                Some(Command::Open(OpenRequest::Unlock {
                    password: Zeroizing::new(password),
                }))
            }
            KeyCode::Esc => {
                self.busy = true;
                Some(Command::Open(OpenRequest::Existing { keys: None }))
            }
            _ => {
                self.field.handle(key);
                None
            }
        }
    }

    /// The worker rejected the password.
    pub fn failed(&mut self, error: &ErrorView) {
        self.busy = false;
        self.field.reset();
        self.field.set_error(error.title.to_lowercase());
    }

    /// Draw the prompt.
    pub fn render(&self, f: &mut Frame, area: Rect, theme: &Theme) {
        let inner = widgets::modal(
            f,
            centered(area, 60, 11),
            "Unlock wallet",
            Tone::Info,
            theme,
        );
        let [text, field, hints] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(FIELD_HEIGHT),
            Constraint::Length(2),
        ])
        .areas(inner);
        f.render_widget(
            widgets::paragraph(Span::styled(
                "The recovery words are encrypted. Enter the password.",
                theme.muted(),
            )),
            text,
        );
        render_field(f, field, &self.field.view(), !self.busy, theme);
        f.render_widget(
            hint_line(
                &[
                    ("enter".into(), "unlock".into()),
                    ("esc".into(), "open watch-only".into()),
                ],
                theme,
            ),
            hints,
        );
    }
}

// === Onboarding

const CHOICES: [&str; 2] = ["Create a new wallet", "Restore from recovery words"];

enum Step {
    Choose {
        selected: usize,
    },
    ShowWords {
        words: Zeroizing<String>,
    },
    Restore {
        focus: usize,
        mnemonic: Field<MnemonicValidator>,
        passphrase: Field<AnyText>,
        birthday: Field<OptionalHeightValidator>,
    },
    Protect {
        focus: usize,
        password: Field<AnyText>,
        confirm: Field<AnyText>,
        keys: KeySource,
        birthday: Option<u32>,
    },
    Creating,
}

/// The create / restore wizard.
pub struct Onboarding {
    step: Step,
    banner: Option<ErrorView>,
    length: MnemonicLength,
    /// Mainnet wallets must be encrypted.
    require_password: bool,
}

impl Onboarding {
    /// Start at the create/restore choice.
    pub fn new(length: MnemonicLength, network: Network) -> Self {
        Self {
            step: Step::Choose { selected: 0 },
            banner: None,
            length,
            require_password: network == Network::Bitcoin,
        }
    }

    /// Name of the current step.
    #[cfg(test)]
    pub fn step_name(&self) -> &'static str {
        match self.step {
            Step::Choose { .. } => "choose",
            Step::ShowWords { .. } => "words",
            Step::Restore { .. } => "restore",
            Step::Protect { .. } => "protect",
            Step::Creating => "creating",
        }
    }

    /// Creating failed; go back to the start with the reason.
    pub fn failed(&mut self, error: &ErrorView) {
        self.banner = Some(error.clone());
        self.step = Step::Choose { selected: 0 };
    }

    fn fresh_words(&self) -> Option<Zeroizing<String>> {
        generate_mnemonic(self.length).ok()
    }

    fn protect(keys: KeySource, birthday: Option<u32>) -> Step {
        Step::Protect {
            focus: 0,
            password: Field::masked("Password (optional)", "leave empty to skip", AnyText),
            confirm: Field::masked("Confirm password", "", AnyText),
            keys,
            birthday,
        }
    }

    /// Handle a key; may produce the create command.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<Command> {
        let require_password = self.require_password;
        let next = match &mut self.step {
            Step::Choose { selected } => match key.code {
                KeyCode::Up | KeyCode::Down => {
                    *selected = 1 - *selected;
                    None
                }
                KeyCode::Enter if *selected == 0 => {
                    self.banner = None;
                    self.fresh_words().map(|words| Step::ShowWords { words })
                }
                KeyCode::Enter => {
                    self.banner = None;
                    Some(Step::Restore {
                        focus: 0,
                        mnemonic: Field::new("Recovery words", "12 to 24 words", MnemonicValidator),
                        passphrase: Field::masked(
                            "Passphrase (optional)",
                            "BIP39 passphrase",
                            AnyText,
                        ),
                        birthday: Field::new(
                            "Birthday height (optional)",
                            "skip older blocks",
                            OptionalHeightValidator,
                        ),
                    })
                }
                _ => None,
            },
            Step::ShowWords { words } => match key.code {
                KeyCode::Char('g') => self.fresh_words().map(|words| Step::ShowWords { words }),
                KeyCode::Esc => Some(Step::Choose { selected: 0 }),
                KeyCode::Enter => Some(Self::protect(KeySource::mnemonic(words.as_str()), None)),
                _ => None,
            },
            Step::Restore {
                focus,
                mnemonic,
                passphrase,
                birthday,
            } => match key.code {
                KeyCode::Esc => Some(Step::Choose { selected: 1 }),
                KeyCode::Tab | KeyCode::Down => {
                    *focus = (*focus + 1) % 3;
                    None
                }
                KeyCode::BackTab | KeyCode::Up => {
                    *focus = (*focus + 2) % 3;
                    None
                }
                KeyCode::Enter => match (mnemonic.submit(), birthday.submit()) {
                    (Ok(words), Ok(height)) => {
                        let pass = passphrase.value().unwrap_or_default();
                        let keys = if pass.is_empty() {
                            KeySource::mnemonic(words)
                        } else {
                            KeySource::mnemonic_with_passphrase(words, pass)
                        };
                        Some(Self::protect(keys, height))
                    }
                    (Err(_), _) => {
                        *focus = 0;
                        None
                    }
                    (_, Err(_)) => {
                        *focus = 2;
                        None
                    }
                },
                _ => {
                    match focus {
                        0 => mnemonic.handle(key),
                        1 => passphrase.handle(key),
                        _ => birthday.handle(key),
                    };
                    None
                }
            },
            Step::Protect {
                focus,
                password,
                confirm,
                keys,
                birthday,
            } => match key.code {
                KeyCode::Esc => Some(Step::Choose { selected: 0 }),
                KeyCode::Tab | KeyCode::Down | KeyCode::BackTab | KeyCode::Up => {
                    *focus = 1 - *focus;
                    None
                }
                KeyCode::Enter => {
                    let pw = password.input.value().to_string();
                    if pw.is_empty() && require_password {
                        password.set_error("required on mainnet");
                        *focus = 0;
                        return None;
                    }
                    if !pw.is_empty() {
                        if let Err(e) = (PasswordValidator {
                            min_len: MIN_PASSWORD,
                        })
                        .validate(&pw)
                        {
                            password.set_error(e);
                            *focus = 0;
                            return None;
                        }
                        if confirm.input.value() != pw {
                            confirm.set_error("passwords do not match");
                            *focus = 1;
                            return None;
                        }
                    }
                    let new = NewWallet {
                        keys: keys.clone(),
                        birthday: *birthday,
                        password: (!pw.is_empty()).then(|| Zeroizing::new(pw)),
                    };
                    self.step = Step::Creating;
                    return Some(Command::Open(OpenRequest::Create(new)));
                }
                _ => {
                    if *focus == 0 {
                        password.handle(key);
                    } else {
                        confirm.handle(key);
                    }
                    None
                }
            },
            Step::Creating => None,
        };
        if let Some(step) = next {
            self.step = step;
        }
        None
    }

    /// Draw the current step.
    pub fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        theme: &Theme,
        now: std::time::Instant,
        since: std::time::Instant,
    ) {
        let inner = widgets::modal(
            f,
            centered(area, 80, 22),
            "Set up your wallet",
            Tone::Info,
            theme,
        );
        let [body, banner] =
            Layout::vertical([Constraint::Min(10), Constraint::Length(2)]).areas(inner);
        if let Some(error) = &self.banner {
            let lines = vec![
                Line::from(Span::styled(
                    format!("✗ {}", error.title),
                    Style::new().fg(theme.error),
                )),
                Line::from(Span::styled(error.hint.clone(), theme.muted())),
            ];
            f.render_widget(widgets::paragraph(lines), banner);
        }
        match &self.step {
            Step::Choose { selected } => {
                let mut lines = vec![
                    Line::from(Span::styled(
                        "No wallet in this data directory yet.",
                        theme.text(),
                    )),
                    Line::raw(""),
                ];
                for (i, choice) in CHOICES.iter().enumerate() {
                    let style = if i == *selected {
                        theme.selected()
                    } else {
                        theme.text()
                    };
                    let marker = if i == *selected { "›" } else { " " };
                    lines.push(Line::from(Span::styled(
                        format!(" {marker} {choice} "),
                        style,
                    )));
                }
                lines.extend([
                    Line::raw(""),
                    hint_line(
                        &[
                            ("↑↓".into(), "choose".into()),
                            ("enter".into(), "continue".into()),
                            ("ctrl+c".into(), "quit".into()),
                        ],
                        theme,
                    ),
                ]);
                f.render_widget(widgets::paragraph(lines), body);
            }
            Step::ShowWords { words } => {
                let list: Vec<&str> = words.split(' ').collect();
                let mut lines = vec![
                    Line::from(Span::styled(
                        format!(
                            "Your {} recovery words. Write them down in order; they are the only backup.",
                            list.len()
                        ),
                        theme.text(),
                    )),
                    Line::raw(""),
                ];
                for (row, chunk) in list.chunks(4).enumerate() {
                    lines.push(Line::from(
                        chunk
                            .iter()
                            .enumerate()
                            .map(|(i, w)| {
                                Span::styled(
                                    format!("{:>3}. {w:<12}", row * 4 + i + 1),
                                    theme.title(),
                                )
                            })
                            .collect::<Vec<_>>(),
                    ));
                }
                lines.extend([
                    Line::raw(""),
                    hint_line(
                        &[
                            ("enter".into(), "I wrote them down".into()),
                            ("g".into(), "new words".into()),
                            ("esc".into(), "back".into()),
                        ],
                        theme,
                    ),
                ]);
                f.render_widget(widgets::paragraph(lines), body);
            }
            Step::Restore {
                focus,
                mnemonic,
                passphrase,
                birthday,
            } => {
                let [a, b, c, hints] = Layout::vertical([
                    Constraint::Length(FIELD_HEIGHT),
                    Constraint::Length(FIELD_HEIGHT),
                    Constraint::Length(FIELD_HEIGHT),
                    Constraint::Length(1),
                ])
                .areas(body);
                render_field(f, a, &mnemonic.view(), *focus == 0, theme);
                render_field(f, b, &passphrase.view(), *focus == 1, theme);
                render_field(f, c, &birthday.view(), *focus == 2, theme);
                f.render_widget(
                    hint_line(
                        &[
                            ("tab".into(), "next".into()),
                            ("enter".into(), "continue".into()),
                            ("esc".into(), "back".into()),
                        ],
                        theme,
                    ),
                    hints,
                );
            }
            Step::Protect {
                focus,
                password,
                confirm,
                ..
            } => {
                let [text, a, b, hints] = Layout::vertical([
                    Constraint::Length(3),
                    Constraint::Length(FIELD_HEIGHT),
                    Constraint::Length(FIELD_HEIGHT),
                    Constraint::Length(1),
                ])
                .areas(body);
                let note = if self.require_password {
                    "Mainnet wallets must be encrypted with a password."
                } else {
                    "Encrypt the recovery words with a password, or leave empty to store them\nreadable only by your user (fine for regtest and signet)."
                };
                f.render_widget(widgets::paragraph(Span::styled(note, theme.muted())), text);
                render_field(f, a, &password.view(), *focus == 0, theme);
                render_field(f, b, &confirm.view(), *focus == 1, theme);
                f.render_widget(
                    hint_line(
                        &[
                            ("tab".into(), "next".into()),
                            ("enter".into(), "create wallet".into()),
                            ("esc".into(), "start over".into()),
                        ],
                        theme,
                    ),
                    hints,
                );
            }
            Step::Creating => {
                let line = Line::from(vec![
                    Span::styled(
                        format!("{} ", super::anim::spinner(since, now)),
                        theme.title(),
                    ),
                    Span::styled("Creating wallet…", theme.text()),
                ]);
                f.render_widget(widgets::paragraph(line), body);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(o: &mut Onboarding, text: &str) {
        for c in text.chars() {
            o.on_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn create_flow_generates_words_then_creates() {
        let mut o = Onboarding::new(MnemonicLength::Words18, Network::Regtest);
        o.on_key(key(KeyCode::Enter));
        assert_eq!(o.step_name(), "words");
        let Step::ShowWords { words } = &o.step else {
            unreachable!()
        };
        assert_eq!(words.split(' ').count(), 18, "configured length is used");
        let first = words.clone();
        o.on_key(key(KeyCode::Char('g')));
        let Step::ShowWords { words } = &o.step else {
            unreachable!()
        };
        assert_ne!(*words, first, "regenerating gives new words");

        o.on_key(key(KeyCode::Enter));
        assert_eq!(o.step_name(), "protect");
        // No password: allowed off mainnet.
        let command = o.on_key(key(KeyCode::Enter));
        assert!(matches!(
            command,
            Some(Command::Open(OpenRequest::Create(NewWallet {
                password: None,
                ..
            })))
        ));
        assert_eq!(o.step_name(), "creating");
    }

    #[test]
    fn passwords_must_be_long_enough_and_match() {
        let mut o = Onboarding::new(MnemonicLength::Words12, Network::Regtest);
        o.on_key(key(KeyCode::Enter));
        o.on_key(key(KeyCode::Enter));
        type_text(&mut o, "short");
        assert!(o.on_key(key(KeyCode::Enter)).is_none());
        let Step::Protect { password, .. } = &o.step else {
            unreachable!()
        };
        assert_eq!(password.error(), Some("at least 8 characters"));

        o.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        type_text(&mut o, "long enough");
        o.on_key(key(KeyCode::Tab));
        type_text(&mut o, "different!");
        assert!(o.on_key(key(KeyCode::Enter)).is_none());
        let Step::Protect { confirm, .. } = &o.step else {
            unreachable!()
        };
        assert_eq!(confirm.error(), Some("passwords do not match"));

        o.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        type_text(&mut o, "long enough");
        let command = o.on_key(key(KeyCode::Enter));
        assert!(matches!(
            command,
            Some(Command::Open(OpenRequest::Create(NewWallet {
                password: Some(_),
                ..
            })))
        ));
    }

    #[test]
    fn mainnet_requires_a_password() {
        let mut o = Onboarding::new(MnemonicLength::Words12, Network::Bitcoin);
        o.on_key(key(KeyCode::Enter));
        o.on_key(key(KeyCode::Enter));
        assert!(o.on_key(key(KeyCode::Enter)).is_none());
        let Step::Protect { password, .. } = &o.step else {
            unreachable!()
        };
        assert_eq!(password.error(), Some("required on mainnet"));
    }

    #[test]
    fn restore_validates_words_and_carries_passphrase_and_birthday() {
        let mut o = Onboarding::new(MnemonicLength::Words12, Network::Regtest);
        o.on_key(key(KeyCode::Down));
        o.on_key(key(KeyCode::Enter));
        assert_eq!(o.step_name(), "restore");

        type_text(&mut o, "not real words");
        o.on_key(key(KeyCode::Enter));
        assert_eq!(o.step_name(), "restore", "invalid words stay on the form");

        let phrase = generate_mnemonic(MnemonicLength::Words12).unwrap();
        o.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        type_text(&mut o, &phrase);
        o.on_key(key(KeyCode::Tab));
        type_text(&mut o, "extra");
        o.on_key(key(KeyCode::Tab));
        type_text(&mut o, "1,200");
        o.on_key(key(KeyCode::Enter));
        let Step::Protect { keys, birthday, .. } = &o.step else {
            panic!("expected protect step, at {}", o.step_name());
        };
        assert_eq!(*birthday, Some(1_200));
        assert!(
            matches!(keys, KeySource::Mnemonic { passphrase: Some(p), .. } if p.as_str() == "extra")
        );
    }

    #[test]
    fn unlock_submits_password_or_goes_watch_only() {
        let mut u = Unlock::default();
        assert!(u.on_key(key(KeyCode::Enter)).is_none(), "empty password");
        for c in "pw".chars() {
            u.on_key(key(KeyCode::Char(c)));
        }
        assert!(matches!(
            u.on_key(key(KeyCode::Enter)),
            Some(Command::Open(OpenRequest::Unlock { password })) if password.as_str() == "pw"
        ));
        assert!(
            u.on_key(key(KeyCode::Enter)).is_none(),
            "busy while unlocking"
        );
        u.failed(&ErrorView::new("Wrong password", ""));
        assert_eq!(u.field.error(), Some("wrong password"));

        let mut u = Unlock::default();
        assert!(matches!(
            u.on_key(key(KeyCode::Esc)),
            Some(Command::Open(OpenRequest::Existing { keys: None }))
        ));
    }
}
