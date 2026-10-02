//! History: every wallet transaction, newest first.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};

use super::{Ctx, Paged, Screen, View};
use crate::tui::anim;
use crate::tui::format;
use crate::tui::message::WorkerEvent;
use crate::tui::modal::Modal;
use crate::tui::theme::Tone;
use crate::tui::widgets::{self, panel};

/// The history tab.
#[derive(Default)]
pub struct History {
    paged: Paged,
}

impl Screen for History {
    fn title(&self) -> &'static str {
        "History"
    }

    fn hints(&self) -> Vec<(String, String)> {
        let mut hints = Paged::hints();
        hints.extend([
            ("enter".into(), "details".into()),
            ("b".into(), "bump fee".into()),
        ]);
        hints
    }

    fn on_key(&mut self, key: KeyEvent, cx: &mut Ctx) {
        let txs = cx.snapshot.map_or(&[][..], |s| &s.txs[..]);
        if self.paged.on_key(key.code, txs.len()) {
            return;
        }
        match key.code {
            KeyCode::Enter => {
                if let Some(tx) = txs.get(self.paged.selected) {
                    *cx.modal = Some(Modal::TxDetail { txid: tx.txid });
                }
            }
            KeyCode::Char('b') => {
                let Some(tx) = txs.get(self.paged.selected) else {
                    return;
                };
                let txid = tx.txid;
                if cx.snapshot.is_some_and(|s| !s.can_sign) {
                    cx.notify(Tone::Warning, "Watch-only wallets cannot bump fees");
                } else {
                    // Confirmed or non-RBF transactions are allowed through on
                    // purpose: the library's typed error explains why it fails.
                    *cx.modal = Some(Modal::bump(txid, cx.config));
                }
            }
            _ => {}
        }
    }

    fn on_event(&mut self, event: &WorkerEvent, cx: &mut Ctx) {
        if let WorkerEvent::Bumped { replacement, .. } = event {
            cx.notify(
                Tone::Success,
                format!(
                    "Fee bumped · replacement {}",
                    format::short(&replacement.to_string(), 6)
                ),
            );
        }
        self.paged.clamp(cx.snapshot.map_or(0, |s| s.txs.len()));
    }

    fn render(&self, f: &mut Frame, area: Rect, view: &View) {
        let theme = view.theme;
        let block = panel("History", theme, true);
        let Some(s) = view.snapshot.filter(|_| view.synced) else {
            let inner = block.inner(area);
            f.render_widget(block, area);
            return widgets::skeleton_block(f, inner, view.loading_since, view.now, theme);
        };
        if s.txs.is_empty() {
            let inner = block.inner(area);
            f.render_widget(block, area);
            let text = Line::from(Span::styled("No transactions yet.", theme.muted()));
            return f.render_widget(Paragraph::new(text), inner);
        }
        // Borders (2) and the header row (1) are not item rows.
        let (page, at) = self
            .paged
            .page(&s.txs, area.height.saturating_sub(3) as usize);
        let block = block.title(
            Line::from(Span::styled(format!(" {} ", page.summary()), theme.muted()))
                .right_aligned(),
        );
        let rows = page.items.iter().map(|tx| {
            let net = format::net(tx);
            let flash = view
                .flashes
                .get(&tx.txid)
                .is_some_and(|t| anim::flashing(*t, view.now));
            let amount_style = Style::new().fg(theme.tone(if net < 0 {
                Tone::Warning
            } else {
                Tone::Success
            }));
            let row = Row::new(vec![
                Cell::from(Span::styled(
                    format::status(&tx.status),
                    theme.status(&tx.status),
                )),
                Cell::from(Span::styled(
                    format!("{:>14}", format::signed_sats(net)),
                    amount_style,
                )),
                Cell::from(Span::styled(
                    format!(
                        "{:>9}",
                        format::paid_fee(tx)
                            .map(|f| format::thousands(f.to_sat()))
                            .unwrap_or_else(|| "-".into())
                    ),
                    theme.muted(),
                )),
                Cell::from(Span::styled(tx.txid.to_string(), theme.text())),
            ]);
            if flash {
                row.style(theme.selected())
            } else {
                row
            }
        });
        let header = Row::new(["status", "net sat", "fee", "txid"]).style(theme.title());
        let table = Table::new(
            rows,
            [
                Constraint::Length(12),
                Constraint::Length(15),
                Constraint::Length(10),
                Constraint::Min(20),
            ],
        )
        .header(header)
        .block(block)
        .row_highlight_style(theme.selected())
        .highlight_symbol("› ");
        let mut state = TableState::default().with_selected(Some(at));
        f.render_stateful_widget(table, area, &mut state);
    }
}
