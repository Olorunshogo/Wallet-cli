//! Dialogs drawn over the main interface.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use wallet::bitcoin::Txid;
use zeroize::Zeroizing;

use super::config::TuiConfig;
use super::format;
use super::message::Command;
use super::screens::View;
use super::theme::Tone;
use super::validate::{FeeRateValidator, WalletNameValidator};
use super::widgets::input::{FIELD_HEIGHT, Field, render_field};
use super::widgets::{self, centered, hint_line};

/// What a key press did to a dialog.
pub enum Outcome {
    /// Stay open.
    Keep,
    /// Close it.
    Close,
    /// Replace it with another dialog.
    Open(Modal),
}

/// An open dialog.
pub enum Modal {
    /// Every key binding.
    Help {
        /// `(key, description)` pairs to list.
        keys: Vec<(String, String)>,
    },
    /// Details of one transaction.
    TxDetail {
        /// Which transaction.
        txid: Txid,
    },
    /// Ask for a new fee rate to replace a transaction.
    BumpFee {
        /// Transaction to replace.
        txid: Txid,
        /// New fee rate input.
        field: Field<FeeRateValidator>,
    },
    /// Pick or create a named wallet.
    Wallets {
        /// The wallets; `None` while loading.
        list: Option<Vec<crate::wallets::Entry>>,
        /// The open wallet.
        current: Option<String>,
        /// Highlighted row.
        selected: usize,
        /// Name input while creating a new wallet.
        naming: Option<Field<WalletNameValidator>>,
    },
    /// Demo start: show the freshly generated recovery words.
    Welcome {
        /// The words.
        mnemonic: Zeroizing<String>,
    },
}

impl Modal {
    /// A fee-bump dialog with bounds from the config.
    pub fn bump(txid: Txid, config: &TuiConfig) -> Self {
        Modal::BumpFee {
            txid,
            field: Field::new(
                "New fee rate (sat/vB)",
                "higher than the original",
                FeeRateValidator {
                    min: 1,
                    max: config.max_fee_rate,
                },
            ),
        }
    }

    /// Handle a key.
    pub fn on_key(
        &mut self,
        key: KeyEvent,
        commands: &mut Vec<Command>,
        config: &TuiConfig,
    ) -> Outcome {
        // Esc first leaves the name input, then the dialog.
        if let Modal::Wallets {
            naming: naming @ Some(_),
            ..
        } = self
            && key.code == KeyCode::Esc
        {
            *naming = None;
            return Outcome::Keep;
        }
        if key.code == KeyCode::Esc {
            return Outcome::Close;
        }
        match self {
            Modal::Wallets {
                list,
                current,
                selected,
                naming,
            } => {
                if let Some(field) = naming {
                    if key.code == KeyCode::Enter {
                        if let Ok(name) = field.submit() {
                            commands.push(Command::SwitchWallet { name });
                            return Outcome::Close;
                        }
                        return Outcome::Keep;
                    }
                    field.handle(key);
                    return Outcome::Keep;
                }
                let len = list.as_ref().map_or(0, Vec::len);
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => {
                        *selected = (*selected + 1).min(len.saturating_sub(1));
                    }
                    KeyCode::Char('n') => {
                        let taken = list
                            .as_ref()
                            .map(|l| l.iter().map(|w| w.name.clone()).collect())
                            .unwrap_or_default();
                        *naming = Some(Field::new(
                            "New wallet name",
                            "e.g. bob",
                            WalletNameValidator { taken },
                        ));
                    }
                    KeyCode::Enter => {
                        if let Some(entry) = list.as_ref().and_then(|l| l.get(*selected)) {
                            if current.as_deref() != Some(entry.name.as_str()) {
                                commands.push(Command::SwitchWallet {
                                    name: entry.name.clone(),
                                });
                            }
                            return Outcome::Close;
                        }
                    }
                    _ => {}
                }
                Outcome::Keep
            }
            Modal::BumpFee { txid, field } => {
                if key.code == KeyCode::Enter {
                    if let Ok(fee_rate) = field.submit() {
                        commands.push(Command::BumpFee {
                            txid: *txid,
                            fee_rate,
                        });
                        return Outcome::Close;
                    }
                    return Outcome::Keep;
                }
                field.handle(key);
                Outcome::Keep
            }
            Modal::TxDetail { txid } if key.code == KeyCode::Char('b') => {
                Outcome::Open(Modal::bump(*txid, config))
            }
            _ if matches!(
                key.code,
                KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('?')
            ) =>
            {
                Outcome::Close
            }
            _ => Outcome::Keep,
        }
    }

    /// Draw over `area`.
    pub fn render(&self, f: &mut Frame, area: Rect, view: &View) {
        let theme = view.theme;
        match self {
            Modal::Help { keys } => {
                let rows = keys.len() as u16 + 4;
                let inner = widgets::modal(f, centered(area, 52, rows), "Keys", Tone::Info, theme);
                let lines: Vec<Line> = keys
                    .iter()
                    .map(|(k, d)| {
                        Line::from(vec![
                            Span::styled(format!("{k:>12}  "), theme.title()),
                            Span::styled(d.clone(), theme.text()),
                        ])
                    })
                    .chain([
                        Line::raw(""),
                        Line::from(Span::styled("        esc to close", theme.muted())),
                    ])
                    .collect();
                f.render_widget(widgets::paragraph(lines), inner);
            }
            Modal::TxDetail { txid } => {
                let inner =
                    widgets::modal(f, centered(area, 76, 13), "Transaction", Tone::Info, theme);
                let tx = view
                    .snapshot
                    .and_then(|s| s.txs.iter().find(|t| t.txid == *txid));
                let lines = match tx {
                    Some(tx) => {
                        let net = format::net(tx);
                        let row = |label: &str, value: Span<'static>| {
                            Line::from(vec![
                                Span::styled(format!("{label:>10}  "), theme.muted()),
                                value,
                            ])
                        };
                        vec![
                            row("txid", Span::styled(tx.txid.to_string(), theme.text())),
                            row(
                                "status",
                                Span::styled(
                                    match tx.status {
                                        wallet::TxStatus::Unconfirmed => {
                                            "unconfirmed (in mempool)".to_string()
                                        }
                                        wallet::TxStatus::Confirmed {
                                            height,
                                            confirmations,
                                        } => {
                                            format!(
                                                "confirmed in block {height}, {confirmations} confirmations"
                                            )
                                        }
                                    },
                                    theme.status(&tx.status),
                                ),
                            ),
                            row(
                                "net",
                                Span::styled(
                                    format!("{} sat", format::signed_sats(net)),
                                    ratatui::style::Style::new().fg(theme.tone(if net < 0 {
                                        Tone::Warning
                                    } else {
                                        Tone::Success
                                    })),
                                ),
                            ),
                            row(
                                "received",
                                Span::styled(format::sats(tx.received), theme.text()),
                            ),
                            row("sent", Span::styled(format::sats(tx.sent), theme.text())),
                            row(
                                "fee",
                                Span::styled(
                                    format::paid_fee(tx)
                                        .map(format::sats)
                                        .unwrap_or_else(|| "none paid by this wallet".into()),
                                    theme.text(),
                                ),
                            ),
                            Line::raw(""),
                            hint_line(
                                &[
                                    ("b".into(), "bump fee".into()),
                                    ("esc".into(), "close".into()),
                                ],
                                theme,
                            ),
                        ]
                    }
                    None => vec![Line::from(Span::styled(
                        "No longer in the wallet (replaced?)",
                        theme.muted(),
                    ))],
                };
                f.render_widget(widgets::paragraph(lines), inner);
            }
            Modal::BumpFee { txid, field } => {
                let inner = widgets::modal(
                    f,
                    centered(area, 64, 10),
                    "Bump fee (RBF)",
                    Tone::Warning,
                    theme,
                );
                let [text, input, hints] = Layout::vertical([
                    Constraint::Length(2),
                    Constraint::Length(FIELD_HEIGHT),
                    Constraint::Length(1),
                ])
                .areas(inner);
                f.render_widget(
                    widgets::paragraph(Line::from(vec![
                        Span::styled("Replace ", theme.muted()),
                        Span::styled(format::short(&txid.to_string(), 10), theme.text()),
                        Span::styled(" with a higher-fee version.", theme.muted()),
                    ])),
                    text,
                );
                render_field(f, input, &field.view(), true, theme);
                f.render_widget(
                    hint_line(
                        &[
                            ("enter".into(), "replace".into()),
                            ("esc".into(), "cancel".into()),
                        ],
                        theme,
                    ),
                    hints,
                );
            }
            Modal::Wallets {
                list,
                current,
                selected,
                naming,
            } => {
                let rows = list.as_ref().map_or(1, |l| l.len().max(1)) as u16;
                let height = rows + 6 + if naming.is_some() { FIELD_HEIGHT } else { 0 };
                let inner =
                    widgets::modal(f, centered(area, 64, height), "Wallets", Tone::Info, theme);
                let [items, input, hints] = Layout::vertical([
                    Constraint::Min(1),
                    Constraint::Length(if naming.is_some() { FIELD_HEIGHT } else { 0 }),
                    Constraint::Length(2),
                ])
                .areas(inner);
                let lines: Vec<Line> = match list {
                    None => vec![Line::from(Span::styled(
                        format!(
                            "{} Loading wallets…",
                            super::anim::spinner(view.loading_since, view.now)
                        ),
                        theme.muted(),
                    ))],
                    Some(l) if l.is_empty() => vec![Line::from(Span::styled(
                        "No wallets yet. Press n to create one.",
                        theme.muted(),
                    ))],
                    Some(l) => l
                        .iter()
                        .enumerate()
                        .map(|(i, w)| {
                            let is_current = current.as_deref() == Some(w.name.as_str());
                            let row_style = if i == *selected {
                                theme.selected()
                            } else {
                                ratatui::style::Style::new()
                            };
                            let balance = w
                                .last_balance_sat
                                .map(|b| format::btc(wallet::bitcoin::Amount::from_sat(b)))
                                .unwrap_or_else(|| "not synced".into());
                            Line::from(vec![
                                Span::styled(
                                    if i == *selected { " › " } else { "   " },
                                    row_style.patch(theme.title()),
                                ),
                                Span::styled(
                                    if is_current { "● " } else { "  " },
                                    row_style.fg(theme.success),
                                ),
                                Span::styled(
                                    format!("{:<16}", w.name),
                                    row_style.patch(theme.title()),
                                ),
                                Span::styled(format!("{balance:>18}  "), row_style.fg(theme.text)),
                                Span::styled(if w.encrypted { "🔒 " } else { "   " }, row_style),
                                Span::styled(
                                    if w.is_default { "default" } else { "" },
                                    row_style.fg(theme.muted),
                                ),
                            ])
                        })
                        .collect(),
                };
                f.render_widget(widgets::paragraph(lines), items);
                if let Some(field) = naming {
                    render_field(f, input, &field.view(), true, theme);
                }
                let keys: Vec<(String, String)> = if naming.is_some() {
                    vec![
                        ("enter".into(), "create".into()),
                        ("esc".into(), "back".into()),
                    ]
                } else {
                    vec![
                        ("enter".into(), "open".into()),
                        ("n".into(), "new wallet".into()),
                        ("esc".into(), "close".into()),
                    ]
                };
                f.render_widget(hint_line(&keys, theme), hints);
            }
            Modal::Welcome { mnemonic } => {
                let inner = widgets::modal(
                    f,
                    centered(area, 78, 14),
                    "Demo wallet ready",
                    Tone::Success,
                    theme,
                );
                let words: Vec<&str> = mnemonic.split(' ').collect();
                let mut lines = vec![
                    Line::from(Span::styled(
                        "A regtest node is running and a new wallet was created with these words:",
                        theme.text(),
                    )),
                    Line::raw(""),
                ];
                for chunk in words.chunks(6).enumerate() {
                    let (row, chunk) = chunk;
                    let spans: Vec<Span> = chunk
                        .iter()
                        .enumerate()
                        .map(|(i, w)| {
                            Span::styled(format!("{:>2}. {w:<10}", row * 6 + i + 1), theme.title())
                        })
                        .collect();
                    lines.push(Line::from(spans));
                }
                lines.extend([
                    Line::raw(""),
                    Line::from(Span::styled(
                        "Press f for the faucet (101 blocks to you), m to mine a block.",
                        theme.muted(),
                    )),
                    Line::from(Span::styled(
                        "Everything is thrown away when you quit.",
                        theme.muted(),
                    )),
                    Line::raw(""),
                    hint_line(&[("enter".into(), "start".into())], theme),
                ]);
                f.render_widget(widgets::paragraph(lines), inner);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyModifiers;
    use wallet::bitcoin::FeeRate;
    use wallet::bitcoin::hashes::Hash;

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn bump_dialog_validates_then_sends() {
        let txid = Txid::from_byte_array([3; 32]);
        let config = TuiConfig::default();
        let mut modal = Modal::bump(txid, &config);
        let mut commands = Vec::new();
        let enter = modal.on_key(key(KeyCode::Enter), &mut commands, &config);
        assert!(matches!(enter, Outcome::Keep), "empty is invalid");
        assert!(commands.is_empty());
        modal.on_key(key(KeyCode::Char('9')), &mut commands, &config);
        let enter = modal.on_key(key(KeyCode::Enter), &mut commands, &config);
        assert!(matches!(enter, Outcome::Close));
        assert!(matches!(
            commands.as_slice(),
            [Command::BumpFee { fee_rate, .. }] if *fee_rate == FeeRate::from_sat_per_vb_u32(9)
        ));
    }

    #[test]
    fn escape_closes_any_dialog() {
        let config = TuiConfig::default();
        let mut modal = Modal::Help { keys: vec![] };
        let esc = modal.on_key(key(KeyCode::Esc), &mut Vec::new(), &config);
        assert!(matches!(esc, Outcome::Close));
    }

    #[test]
    fn detail_dialog_leads_to_bump() {
        let config = TuiConfig::default();
        let txid = Txid::from_byte_array([4; 32]);
        let mut modal = Modal::TxDetail { txid };
        let next = modal.on_key(key(KeyCode::Char('b')), &mut Vec::new(), &config);
        assert!(matches!(next, Outcome::Open(Modal::BumpFee { txid: t, .. }) if t == txid));
    }
}
