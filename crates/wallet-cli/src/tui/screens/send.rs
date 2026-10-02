//! Send: a validated form, a signed preview, then broadcast.

use std::time::Instant;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use wallet::bitcoin::{Amount, Network};
use wallet::{CoinSelection, Recipient};

use super::{Ctx, Screen, View};
use crate::tui::anim;
use crate::tui::config::TuiConfig;
use crate::tui::format::{self, ErrorView};
use crate::tui::message::{Command, Op, SendPreview, WorkerEvent};
use crate::tui::theme::Tone;
use crate::tui::validate::{AddressValidator, AmountValidator, FeeRateValidator};
use crate::tui::widgets::input::{FIELD_HEIGHT, Field, render_field};
use crate::tui::widgets::{self, hint_line, panel};

/// Coin selection choices, in display order. `None` keeps the wallet default.
pub const SELECTIONS: [(Option<CoinSelection>, &str); 4] = [
    (None, "wallet default"),
    (Some(CoinSelection::BranchAndBound), "branch and bound"),
    (Some(CoinSelection::LargestFirst), "largest first"),
    (Some(CoinSelection::OldestFirst), "oldest first"),
];

/// Focus positions in the form.
const ADDRESS: usize = 0;
const AMOUNT: usize = 1;
const FEE: usize = 2;
const SELECTION: usize = 3;
const FOCUSABLE: usize = 4;

/// Where the payment is in its lifecycle.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    /// Filling in the form.
    Editing,
    /// Waiting for the worker to build and sign.
    Previewing(Instant),
    /// Signed; waiting for the user to confirm.
    Review(SendPreview),
    /// Broadcasting this amount.
    Sending(Instant, Amount),
}

/// The send tab.
pub struct Send {
    address: Field<AddressValidator>,
    amount: Field<AmountValidator>,
    fee: Field<FeeRateValidator>,
    selection: usize,
    focus: usize,
    /// True while keys go to the form; Esc leaves the form.
    editing: bool,
    stage: Stage,
    estimating: bool,
    banner: Option<ErrorView>,
    fee_target: u16,
    /// Whether the node is reachable; offline payments are saved, not sent.
    online: bool,
}

impl Send {
    /// A blank form for `network`.
    pub fn new(network: Network, config: &TuiConfig) -> Self {
        Self {
            address: Field::new("Pay to", "address", AddressValidator { network }),
            amount: Field::new(
                "Amount",
                "e.g. 25000 or 0.00025 btc",
                AmountValidator::default(),
            ),
            fee: Field::new(
                "Fee rate (sat/vB)",
                "ctrl+e to estimate",
                FeeRateValidator {
                    min: 1,
                    max: config.max_fee_rate,
                },
            ),
            selection: 0,
            focus: ADDRESS,
            editing: false,
            stage: Stage::Editing,
            estimating: false,
            banner: None,
            fee_target: config.fee_target_blocks,
            online: true,
        }
    }

    /// The current stage.
    #[cfg(test)]
    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    fn reset(&mut self) {
        self.address.reset();
        self.amount.reset();
        self.focus = ADDRESS;
        self.editing = false;
        self.stage = Stage::Editing;
    }

    fn estimate(&mut self, cx: &mut Ctx) {
        self.estimating = true;
        cx.send(Command::EstimateFee {
            target: self.fee_target,
        });
    }

    fn submit(&mut self, cx: &mut Ctx) {
        self.amount.validator.max = cx
            .snapshot
            .map(|s| s.balance.confirmed + s.balance.unconfirmed);
        let address = self.address.submit();
        let amount = self.amount.submit();
        let fee = self.fee.submit();
        match (address, amount, fee) {
            (Ok(address), Ok(amount), Ok(fee_rate)) => {
                self.banner = None;
                self.stage = Stage::Previewing(cx.now);
                cx.send(Command::PreviewSend {
                    recipient: Recipient::new(address, amount),
                    fee_rate,
                    selection: SELECTIONS[self.selection].0,
                });
            }
            (address, amount, _) => {
                self.focus = if address.is_err() {
                    ADDRESS
                } else if amount.is_err() {
                    AMOUNT
                } else {
                    FEE
                };
            }
        }
    }

    fn edit_key(&mut self, key: KeyEvent, cx: &mut Ctx) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.editing = false,
            KeyCode::Char('e') if ctrl => self.estimate(cx),
            KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % FOCUSABLE,
            KeyCode::BackTab | KeyCode::Up => self.focus = (self.focus + FOCUSABLE - 1) % FOCUSABLE,
            KeyCode::Enter => self.submit(cx),
            KeyCode::Left if self.focus == SELECTION => {
                self.selection = (self.selection + SELECTIONS.len() - 1) % SELECTIONS.len();
            }
            KeyCode::Right if self.focus == SELECTION => {
                self.selection = (self.selection + 1) % SELECTIONS.len();
            }
            _ => {
                match self.focus {
                    ADDRESS => self.address.handle(key),
                    AMOUNT => self.amount.handle(key),
                    FEE => self.fee.handle(key),
                    _ => false,
                };
            }
        }
    }
}

impl Screen for Send {
    fn title(&self) -> &'static str {
        "Send"
    }

    fn hints(&self) -> Vec<(String, String)> {
        let pairs: &[(&str, &str)] = match (&self.stage, self.editing) {
            (Stage::Editing, false) => &[("enter", "fill in")],
            (Stage::Editing, true) => &[
                ("tab", "next field"),
                ("ctrl+e", "estimate fee"),
                ("enter", "review"),
                ("esc", "leave form"),
            ],
            (Stage::Review(_), _) if self.online => &[("y/enter", "send"), ("n/esc", "cancel")],
            (Stage::Review(_), _) => &[("y/enter", "save for later"), ("n/esc", "cancel")],
            _ => &[],
        };
        pairs
            .iter()
            .map(|(k, d)| (k.to_string(), d.to_string()))
            .collect()
    }

    fn captures_input(&self) -> bool {
        self.editing && self.stage == Stage::Editing
    }

    fn on_show(&mut self, cx: &mut Ctx) {
        self.online = cx.online;
        if self.fee.input.value().is_empty() && !self.estimating {
            self.estimate(cx);
        }
    }

    fn on_key(&mut self, key: KeyEvent, cx: &mut Ctx) {
        self.online = cx.online;
        match &self.stage {
            Stage::Editing if self.editing => self.edit_key(key, cx),
            Stage::Editing => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Char('i')) {
                    self.editing = true;
                }
            }
            Stage::Review(preview) => match key.code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    self.stage = Stage::Sending(cx.now, preview.amount);
                    cx.send(Command::ConfirmSend);
                }
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.stage = Stage::Editing;
                    cx.send(Command::CancelSend);
                }
                _ => {}
            },
            Stage::Previewing(_) | Stage::Sending(..) => {}
        }
    }

    fn on_event(&mut self, event: &WorkerEvent, cx: &mut Ctx) {
        self.online = cx.online;
        match event {
            WorkerEvent::FeeEstimate { rate, fallback } => {
                self.estimating = false;
                let sat_vb = rate.to_sat_per_vb_ceil();
                self.fee.set(sat_vb.to_string());
                if *fallback {
                    cx.notify(
                        Tone::Warning,
                        format!("No fee data on the node yet; using {sat_vb} sat/vB"),
                    );
                }
            }
            WorkerEvent::Preview(preview) => self.stage = Stage::Review(preview.clone()),
            // The app announces where it was saved.
            WorkerEvent::Saved { .. } => self.reset(),
            WorkerEvent::Sent { txid, fee, .. } => {
                let amount = match &self.stage {
                    Stage::Sending(_, amount) => *amount,
                    _ => self.amount.value().unwrap_or(Amount::ZERO),
                };
                cx.notify(
                    Tone::Success,
                    format!(
                        "Sent {} (fee {}) · {}",
                        format::sats(amount),
                        format::sats(*fee),
                        format::short(&txid.to_string(), 6)
                    ),
                );
                self.reset();
            }
            WorkerEvent::Failed {
                op: Op::Preview | Op::Send | Op::FeeEstimate,
                error,
            } => {
                self.estimating = false;
                self.stage = Stage::Editing;
                self.editing = true;
                self.banner = Some(error.clone());
            }
            _ => {}
        }
    }

    fn render(&self, f: &mut Frame, area: Rect, view: &View) {
        let theme = view.theme;
        let [form_area, side] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(area);

        let block = panel("New payment", theme, self.editing);
        let inner = block.inner(form_area);
        f.render_widget(block, form_area);
        let [address, amount, fee, selection, available, banner] = Layout::vertical([
            Constraint::Length(FIELD_HEIGHT),
            Constraint::Length(FIELD_HEIGHT),
            Constraint::Length(FIELD_HEIGHT),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(2),
        ])
        .areas(inner);

        let focused = |i: usize| self.editing && self.stage == Stage::Editing && self.focus == i;
        render_field(f, address, &self.address.view(), focused(ADDRESS), theme);
        render_field(f, amount, &self.amount.view(), focused(AMOUNT), theme);
        let mut fee_view = self.fee.view();
        let estimating_text;
        if self.estimating {
            estimating_text = format!(
                "{} estimating…",
                anim::spinner(view.loading_since, view.now)
            );
            fee_view.placeholder = &estimating_text;
        }
        render_field(f, fee, &fee_view, focused(FEE), theme);

        let selector = Line::from(vec![
            Span::styled(" Coin selection  ", theme.muted()),
            Span::styled("‹ ", theme.muted()),
            Span::styled(
                SELECTIONS[self.selection].1,
                if focused(SELECTION) {
                    theme.title()
                } else {
                    theme.text()
                },
            ),
            Span::styled(" ›", theme.muted()),
        ]);
        f.render_widget(widgets::paragraph(selector), selection);

        if let Some(s) = view.snapshot {
            let spendable = s.balance.confirmed + s.balance.unconfirmed;
            f.render_widget(
                widgets::paragraph(Line::from(Span::styled(
                    format!(" Available {}", format::sats(spendable)),
                    theme.muted(),
                ))),
                available,
            );
        }
        if let Some(error) = &self.banner {
            let lines = vec![
                Line::from(Span::styled(
                    format!(" ✗ {}", error.title),
                    Style::new().fg(theme.error),
                )),
                Line::from(Span::styled(format!("   {}", error.hint), theme.muted())),
            ];
            f.render_widget(widgets::paragraph(lines), banner);
        } else if view.snapshot.is_some_and(|s| !s.can_sign) {
            f.render_widget(
                widgets::paragraph(Line::from(Span::styled(
                    " Watch-only wallet: payments can be built but not signed.",
                    Style::new().fg(theme.warning),
                ))),
                banner,
            );
        }

        self.render_side(f, side, view);
    }
}

impl Send {
    fn render_side(&self, f: &mut Frame, area: Rect, view: &View) {
        let theme = view.theme;
        let (title, tone) = match self.stage {
            Stage::Review(_) => ("Review", true),
            _ => ("Status", false),
        };
        let block = panel(title, theme, tone);
        let inner = block.inner(area);
        f.render_widget(block, area);

        let busy = |since: Instant, text: &str| {
            Line::from(vec![
                Span::styled(
                    format!("{} ", anim::spinner(since, view.now)),
                    theme.title(),
                ),
                Span::styled(text.to_string(), theme.text()),
            ])
        };
        let lines = match &self.stage {
            Stage::Editing => vec![
                Line::from(Span::styled(
                    "Fill in the form, then press enter to review.",
                    theme.muted(),
                )),
                Line::raw(""),
                Line::from(Span::styled(
                    "The payment is built and signed first, so you see the",
                    theme.muted(),
                )),
                Line::from(Span::styled(
                    "exact fee and size before anything is broadcast.",
                    theme.muted(),
                )),
                Line::raw(""),
                Line::from(Span::styled(
                    if self.online {
                        "Online: confirming broadcasts the payment."
                    } else {
                        "Offline: confirming signs the payment and saves it to the outbox; it is sent automatically when the node is back."
                    },
                    theme.muted(),
                )),
            ],
            Stage::Previewing(since) => vec![busy(*since, "Selecting coins and signing…")],
            Stage::Sending(since, _) => vec![busy(
                *since,
                if self.online {
                    "Broadcasting…"
                } else {
                    "Saving to outbox…"
                },
            )],
            Stage::Review(p) => {
                let row = |label: &str, value: String, style: Style| {
                    Line::from(vec![
                        Span::styled(format!("{label:<10}"), theme.muted()),
                        Span::styled(value, style),
                    ])
                };
                let total = p.amount + p.fee;
                vec![
                    row("to", format::short(&p.address, 12), theme.text()),
                    row("amount", format::sats(p.amount), theme.title()),
                    row(
                        "fee",
                        format!(
                            "{} ({} sat/vB, {} vB)",
                            format::sats(p.fee),
                            p.fee_rate.to_sat_per_vb_ceil(),
                            p.vsize
                        ),
                        theme.text(),
                    ),
                    row("inputs", p.inputs.to_string(), theme.text()),
                    row(
                        "change",
                        p.change.map(format::sats).unwrap_or_else(|| "none".into()),
                        theme.text(),
                    ),
                    row("total", format::sats(total), Style::new().fg(theme.warning)),
                    Line::raw(""),
                    hint_line(
                        &[
                            (
                                "y".into(),
                                if self.online {
                                    "send"
                                } else {
                                    "save for later"
                                }
                                .into(),
                            ),
                            ("n".into(), "cancel".into()),
                        ],
                        theme,
                    ),
                ]
            }
        };
        f.render_widget(widgets::paragraph(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyModifiers;
    use wallet::bitcoin::FeeRate;

    use super::*;
    use crate::tui::modal::Modal;

    struct Bench {
        commands: Vec<Command>,
        notices: Vec<(Tone, String)>,
        modal: Option<Modal>,
        config: TuiConfig,
    }

    impl Bench {
        fn new() -> Self {
            Self {
                commands: Vec::new(),
                notices: Vec::new(),
                modal: None,
                config: TuiConfig::default(),
            }
        }

        fn cx(&mut self) -> Ctx<'_> {
            Ctx {
                snapshot: None,
                config: &self.config,
                online: true,
                commands: &mut self.commands,
                notices: &mut self.notices,
                modal: &mut self.modal,
                now: Instant::now(),
            }
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(send: &mut Send, bench: &mut Bench, text: &str) {
        for c in text.chars() {
            send.on_key(key(KeyCode::Char(c)), &mut bench.cx());
        }
    }

    fn regtest_address() -> String {
        let phrase = wallet::generate_mnemonic(wallet::MnemonicLength::Words12).unwrap();
        let mut w = wallet::Wallet::builder(Network::Regtest)
            .keys(wallet::KeySource::mnemonic(phrase.as_str()))
            .create()
            .unwrap();
        w.new_address().unwrap().address.to_string()
    }

    #[test]
    fn showing_the_screen_requests_a_fee_estimate_once() {
        let mut bench = Bench::new();
        let mut send = Send::new(Network::Regtest, &bench.config);
        send.on_show(&mut bench.cx());
        send.on_show(&mut bench.cx());
        assert_eq!(bench.commands.len(), 1);
        assert!(matches!(
            bench.commands[0],
            Command::EstimateFee { target: 6 }
        ));
    }

    #[test]
    fn form_only_captures_keys_while_editing() {
        let mut bench = Bench::new();
        let mut send = Send::new(Network::Regtest, &bench.config);
        assert!(!send.captures_input());
        send.on_key(key(KeyCode::Enter), &mut bench.cx());
        assert!(send.captures_input());
        send.on_key(key(KeyCode::Esc), &mut bench.cx());
        assert!(!send.captures_input());
    }

    #[test]
    fn invalid_form_focuses_the_first_problem() {
        let mut bench = Bench::new();
        let mut send = Send::new(Network::Regtest, &bench.config);
        send.on_key(key(KeyCode::Enter), &mut bench.cx());
        type_text(&mut send, &mut bench, "not-an-address");
        send.on_key(key(KeyCode::Enter), &mut bench.cx());
        assert!(bench.commands.is_empty());
        assert_eq!(send.focus, ADDRESS);
        assert_eq!(send.address.error(), Some("not a valid bitcoin address"));
    }

    #[test]
    fn full_flow_from_form_to_sent() {
        let mut bench = Bench::new();
        let mut send = Send::new(Network::Regtest, &bench.config);
        send.on_key(key(KeyCode::Enter), &mut bench.cx());
        type_text(&mut send, &mut bench, &regtest_address());
        send.on_key(key(KeyCode::Tab), &mut bench.cx());
        type_text(&mut send, &mut bench, "25000");
        send.on_event(
            &WorkerEvent::FeeEstimate {
                rate: FeeRate::from_sat_per_vb_u32(4),
                fallback: true,
            },
            &mut bench.cx(),
        );
        assert_eq!(send.fee.input.value(), "4");
        assert_eq!(bench.notices[0].0, Tone::Warning, "fallback is announced");

        // Pick "largest first".
        send.on_key(key(KeyCode::Tab), &mut bench.cx());
        send.on_key(key(KeyCode::Tab), &mut bench.cx());
        send.on_key(key(KeyCode::Right), &mut bench.cx());
        send.on_key(key(KeyCode::Right), &mut bench.cx());
        send.on_key(key(KeyCode::Enter), &mut bench.cx());
        let Some(Command::PreviewSend {
            recipient,
            fee_rate,
            selection,
        }) = bench.commands.pop()
        else {
            panic!("expected a preview request");
        };
        assert_eq!(recipient.amount, Amount::from_sat(25_000));
        assert_eq!(fee_rate, FeeRate::from_sat_per_vb_u32(4));
        assert_eq!(selection, Some(CoinSelection::LargestFirst));
        assert!(matches!(send.stage(), Stage::Previewing(_)));

        let preview = SendPreview {
            address: "bcrt1q".into(),
            amount: Amount::from_sat(25_000),
            fee: Amount::from_sat(564),
            fee_rate: FeeRate::from_sat_per_vb_u32(4),
            vsize: 141,
            inputs: 1,
            change: Some(Amount::from_sat(74_436)),
        };
        send.on_event(&WorkerEvent::Preview(preview), &mut bench.cx());
        send.on_key(key(KeyCode::Char('y')), &mut bench.cx());
        assert!(matches!(bench.commands.pop(), Some(Command::ConfirmSend)));
        assert!(matches!(send.stage(), Stage::Sending(_, a) if *a == Amount::from_sat(25_000)));
    }

    #[test]
    fn cancelling_review_returns_to_the_form() {
        let mut bench = Bench::new();
        let mut send = Send::new(Network::Regtest, &bench.config);
        send.stage = Stage::Review(SendPreview {
            address: String::new(),
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            fee_rate: FeeRate::ZERO,
            vsize: 0,
            inputs: 0,
            change: None,
        });
        send.on_key(key(KeyCode::Char('n')), &mut bench.cx());
        assert!(matches!(bench.commands.pop(), Some(Command::CancelSend)));
        assert_eq!(send.stage(), &Stage::Editing);
    }

    #[test]
    fn worker_failures_show_a_banner() {
        let mut bench = Bench::new();
        let mut send = Send::new(Network::Regtest, &bench.config);
        send.stage = Stage::Previewing(Instant::now());
        send.on_event(
            &WorkerEvent::Failed {
                op: Op::Preview,
                error: ErrorView::new("Insufficient funds", "Short by 1 sat."),
            },
            &mut bench.cx(),
        );
        assert_eq!(send.stage(), &Stage::Editing);
        assert_eq!(send.banner.as_ref().unwrap().title, "Insufficient funds");
    }
}
