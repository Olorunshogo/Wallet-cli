//! UTXOs: the coins the wallet can spend.

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use wallet::Keychain;

use super::{Ctx, Paged, Screen, View};
use crate::tui::format;
use crate::tui::message::WorkerEvent;
use crate::tui::widgets::{self, panel};

/// The UTXO tab.
#[derive(Default)]
pub struct Utxos {
    paged: Paged,
}

impl Screen for Utxos {
    fn title(&self) -> &'static str {
        "UTXOs"
    }

    fn hints(&self) -> Vec<(String, String)> {
        Paged::hints()
    }

    fn on_key(&mut self, key: KeyEvent, cx: &mut Ctx) {
        self.paged
            .on_key(key.code, cx.snapshot.map_or(0, |s| s.utxos.len()));
    }

    fn on_event(&mut self, _event: &WorkerEvent, cx: &mut Ctx) {
        self.paged.clamp(cx.snapshot.map_or(0, |s| s.utxos.len()));
    }

    fn render(&self, f: &mut Frame, area: Rect, view: &View) {
        let theme = view.theme;
        let block = panel("Unspent outputs", theme, true);
        let Some(s) = view.snapshot.filter(|_| view.synced) else {
            let inner = block.inner(area);
            f.render_widget(block, area);
            return widgets::skeleton_block(f, inner, view.loading_since, view.now, theme);
        };
        if s.utxos.is_empty() {
            let inner = block.inner(area);
            f.render_widget(block, area);
            let text = Line::from(Span::styled("No coins yet.", theme.muted()));
            return f.render_widget(Paragraph::new(text), inner);
        }
        // Borders (2) and the header row (1) are not item rows.
        let (page, at) = self
            .paged
            .page(&s.utxos, area.height.saturating_sub(3) as usize);
        let block = block.title(
            Line::from(Span::styled(format!(" {} ", page.summary()), theme.muted()))
                .right_aligned(),
        );
        let rows = page.items.iter().map(|u| {
            let keychain = match u.keychain {
                Keychain::External => "receive",
                Keychain::Internal => "change",
            };
            Row::new(vec![
                Cell::from(Span::styled(
                    format!("{:>14}", format::thousands(u.value.to_sat())),
                    theme.title(),
                )),
                Cell::from(Span::styled(
                    format!("{keychain} #{}", u.derivation_index),
                    theme.muted(),
                )),
                Cell::from(Span::styled(
                    format::status(&u.status),
                    theme.status(&u.status),
                )),
                Cell::from(Span::styled(u.outpoint.to_string(), theme.text())),
            ])
        });
        let header = Row::new(["sat", "address", "status", "outpoint"]).style(theme.title());
        let table = Table::new(
            rows,
            [
                Constraint::Length(15),
                Constraint::Length(14),
                Constraint::Length(12),
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
