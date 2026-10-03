//! Receive: the current address and its QR code.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Ctx, Screen, View};
use crate::tui::message::{Command, WorkerEvent};
use crate::tui::theme::Tone;
use crate::tui::widgets::{self, panel, qr_lines};

/// The receive tab.
pub struct Receive;

impl Screen for Receive {
    fn title(&self) -> &'static str {
        "Receive"
    }

    fn hints(&self) -> Vec<(String, String)> {
        vec![
            ("n".into(), "new address".into()),
            ("c".into(), "copy address".into()),
        ]
    }

    fn on_key(&mut self, key: KeyEvent, cx: &mut Ctx) {
        match key.code {
            KeyCode::Char('n') => cx.send(Command::RevealAddress),
            KeyCode::Char('c') => {
                if let Some(s) = cx.snapshot {
                    cx.copy(s.receive.address.to_string(), "address");
                }
            }
            _ => {}
        }
    }

    fn on_event(&mut self, event: &WorkerEvent, cx: &mut Ctx) {
        if let WorkerEvent::Address(info) = event {
            cx.notify(Tone::Info, format!("New address #{}", info.index));
        }
    }

    fn render(&self, f: &mut Frame, area: Rect, view: &View) {
        let theme = view.theme;
        let block = panel("Receive", theme, true);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let Some(s) = view.snapshot else {
            return widgets::skeleton_block(f, inner, view.loading_since, view.now, theme);
        };
        let address = s.receive.address.to_string();
        let qr = qr_lines(&address);
        let qr_height = qr.len() as u16;
        let [top, code, bottom] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Length(qr_height),
            Constraint::Min(1),
        ])
        .areas(inner);

        let header = vec![
            Line::from(Span::styled(
                format!("Address #{} (BIP84, {})", s.receive.index, s.network),
                theme.muted(),
            )),
            Line::raw(""),
            Line::from(Span::styled(address.clone(), theme.title())),
        ];
        f.render_widget(Paragraph::new(header).alignment(Alignment::Center), top);

        if code.height >= qr_height
            && code.width as usize >= qr.first().map_or(0, |l| l.chars().count())
        {
            let lines: Vec<Line> = qr
                .into_iter()
                .map(|l| Line::styled(l, theme.text()))
                .collect();
            f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), code);
        } else {
            let note = Line::from(Span::styled(
                "(enlarge the terminal to see the QR code)",
                theme.muted(),
            ));
            f.render_widget(Paragraph::new(note).alignment(Alignment::Center), code);
        }
        let note = Line::from(Span::styled(
            "This address stays the same until it receives a payment.",
            theme.muted(),
        ));
        f.render_widget(Paragraph::new(note).alignment(Alignment::Center), bottom);
    }
}
