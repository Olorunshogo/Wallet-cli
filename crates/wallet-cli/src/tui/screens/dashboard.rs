//! Overview: balances, chain status and recent activity.

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use wallet::bitcoin::Amount;

use super::{Ctx, Screen, View};
use crate::tui::anim;
use crate::tui::format;
use crate::tui::theme::Tone;
use crate::tui::widgets::{self, panel, skeleton, stat_card};

/// The dashboard tab.
pub struct Dashboard;

impl Screen for Dashboard {
    fn title(&self) -> &'static str {
        "Dashboard"
    }

    fn hints(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    fn on_key(&mut self, _key: KeyEvent, _cx: &mut Ctx) {}

    fn render(&self, f: &mut Frame, area: Rect, view: &View) {
        let theme = view.theme;
        let [cards, lower] =
            Layout::vertical([Constraint::Length(6), Constraint::Min(6)]).areas(area);
        let [confirmed, unconfirmed, immature, total] =
            Layout::horizontal([Constraint::Ratio(1, 4); 4]).areas(cards);

        let loading = !view.synced;
        let card = |f: &mut Frame, area: Rect, label: &str, sats: u64, color, caption: &str| {
            let value = if loading {
                skeleton(14, view.loading_since, view.now, theme)
            } else {
                Line::from(Span::styled(
                    format::sats(Amount::from_sat(sats)),
                    Style::new()
                        .fg(color)
                        .add_modifier(ratatui::style::Modifier::BOLD),
                ))
            };
            stat_card(f, area, label, value, caption, theme);
        };
        let b = view.balances;
        card(
            f,
            confirmed,
            "Confirmed",
            b.confirmed,
            theme.confirmed,
            "spendable now",
        );
        card(
            f,
            unconfirmed,
            "Unconfirmed",
            b.unconfirmed,
            theme.unconfirmed,
            "in the mempool",
        );
        card(
            f,
            immature,
            "Immature",
            b.immature,
            theme.immature,
            "coinbase < 100 conf",
        );
        let sum = b.confirmed + b.unconfirmed + b.immature;
        card(
            f,
            total,
            "Total",
            sum,
            theme.accent,
            &format::btc(Amount::from_sat(sum)),
        );

        let [info, recent] =
            Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)])
                .areas(lower);
        self.render_info(f, info, view);
        self.render_recent(f, recent, view);
    }
}

impl Dashboard {
    fn render_info(&self, f: &mut Frame, area: Rect, view: &View) {
        let theme = view.theme;
        let block = panel("Wallet", theme, false);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let Some(s) = view.snapshot else {
            return widgets::skeleton_block(f, inner, view.loading_since, view.now, theme);
        };
        let row = |label: &str, value: Span<'static>| {
            Line::from(vec![
                Span::styled(format!("{label:<9}"), theme.muted()),
                value,
            ])
        };
        let (mode, tone) = if s.can_sign {
            ("signing", Tone::Success)
        } else {
            ("watch-only", Tone::Warning)
        };
        let lines = vec![
            row(
                "network",
                Span::styled(format!(" {} ", s.network), theme.network(s.network)),
            ),
            row(
                "mode",
                Span::styled(mode, Style::new().fg(theme.tone(tone))),
            ),
            row(
                "tip",
                Span::styled(format!("{}", s.tip.height), theme.text()),
            ),
            row(
                "block",
                Span::styled(format::short(&s.tip.hash.to_string(), 8), theme.muted()),
            ),
            row(
                "coins",
                Span::styled(s.utxos.len().to_string(), theme.text()),
            ),
            row("txs", Span::styled(s.txs.len().to_string(), theme.text())),
            row(
                "node",
                if view.online {
                    Span::styled(view.chain_label.to_string(), Style::new().fg(theme.success))
                } else {
                    Span::styled(
                        "offline (showing stored data)",
                        Style::new().fg(theme.warning),
                    )
                },
            ),
            row(
                "outbox",
                if view.outbox > 0 {
                    Span::styled(
                        format!("{} saved payment(s) waiting", view.outbox),
                        Style::new().fg(theme.warning),
                    )
                } else {
                    Span::styled("empty", theme.muted())
                },
            ),
        ];
        f.render_widget(widgets::paragraph(lines), inner);
    }

    fn render_recent(&self, f: &mut Frame, area: Rect, view: &View) {
        let theme = view.theme;
        let block = panel("Recent activity", theme, false);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let Some(s) = view.snapshot.filter(|_| view.synced) else {
            return widgets::skeleton_block(f, inner, view.loading_since, view.now, theme);
        };
        if s.txs.is_empty() {
            let hint = Line::from(Span::styled(
                "No transactions yet. Open Receive (2) to get an address.",
                theme.muted(),
            ));
            return f.render_widget(Paragraph::new(hint), inner);
        }
        let lines: Vec<Line> = s
            .txs
            .iter()
            .take(view.config.recent_txs)
            .map(|tx| {
                let net = format::net(tx);
                let flash = view
                    .flashes
                    .get(&tx.txid)
                    .is_some_and(|t| anim::flashing(*t, view.now));
                let base = if flash {
                    theme.selected()
                } else {
                    Style::new()
                };
                Line::from(vec![
                    Span::styled(
                        if net < 0 { " ↑ " } else { " ↓ " },
                        base.fg(theme.tone(if net < 0 {
                            Tone::Warning
                        } else {
                            Tone::Success
                        })),
                    ),
                    Span::styled(
                        format!("{:>14} sat  ", format::signed_sats(net)),
                        base.fg(theme.text),
                    ),
                    Span::styled(
                        format!("{:<12}", format::status(&tx.status)),
                        base.patch(theme.status(&tx.status)),
                    ),
                    Span::styled(format::short(&tx.txid.to_string(), 8), base.fg(theme.muted)),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }
}
