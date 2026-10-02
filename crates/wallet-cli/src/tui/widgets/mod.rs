//! Reusable drawing helpers. Screens compose these instead of styling raw
//! widgets, so the look stays consistent and lives in one place.

pub mod input;

use std::time::Instant;

use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};

use super::anim;
use super::theme::{Theme, Tone};

/// A rounded panel with a title.
pub fn panel<'a>(title: &'a str, theme: &Theme, focused: bool) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.border(focused))
        .title(Span::styled(format!(" {title} "), theme.title()))
}

/// A `width` x `height` rectangle centered in `area`, clamped to fit.
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// A placeholder bar with a moving shine, shown while data loads.
pub fn skeleton(width: u16, start: Instant, now: Instant, theme: &Theme) -> Line<'static> {
    let spans: Vec<Span> = anim::shimmer(width, start, now)
        .into_iter()
        .map(|lit| {
            let color = if lit {
                theme.skeleton_shine
            } else {
                theme.skeleton
            };
            Span::styled("█", Style::new().fg(color))
        })
        .collect();
    Line::from(spans)
}

/// Several skeleton lines of varying width, like loading paragraphs.
pub fn skeleton_block(f: &mut Frame, area: Rect, start: Instant, now: Instant, theme: &Theme) {
    let widths = [90u16, 70, 80, 55, 75];
    let lines: Vec<Line> = (0..area.height)
        .map(|i| {
            let pct = widths[usize::from(i) % widths.len()];
            skeleton(area.width * pct / 100, start, now, theme)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

/// A big-number card: label, value in a color, optional caption.
pub fn stat_card(
    f: &mut Frame,
    area: Rect,
    label: &str,
    value: Line<'_>,
    caption: &str,
    theme: &Theme,
) {
    let block = panel(label, theme, false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let text = Text::from(vec![
        value.alignment(Alignment::Center),
        Line::from(Span::styled(caption.to_string(), theme.muted())).alignment(Alignment::Center),
    ]);
    let [_, body] = Layout::vertical([
        Constraint::Length(inner.height.saturating_sub(2) / 2),
        Constraint::Min(2),
    ])
    .areas(inner);
    f.render_widget(Paragraph::new(text), body);
}

/// A QR code for `data`, drawn with half-block characters.
pub fn qr_lines(data: &str) -> Vec<String> {
    match QrCode::new(data.as_bytes()) {
        Ok(code) => code
            .render::<Dense1x2>()
            .dark_color(Dense1x2::Light)
            .light_color(Dense1x2::Dark)
            .quiet_zone(true)
            .build()
            .lines()
            .map(str::to_string)
            .collect(),
        Err(_) => vec!["(QR unavailable)".into()],
    }
}

/// A modal: clears what is behind it and draws a bordered box.
pub fn modal(f: &mut Frame, area: Rect, title: &str, tone: Tone, theme: &Theme) -> Rect {
    // Clear resets cells to the terminal default; repaint the dark background.
    f.render_widget(Clear, area);
    f.render_widget(Block::new().style(theme.base()), area);
    let block = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::new().fg(theme.tone(tone)))
        .title(Span::styled(
            format!(" {title} "),
            Style::new()
                .fg(theme.tone(tone))
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

/// Wrapped paragraph helper.
pub fn paragraph<'a>(text: impl Into<Text<'a>>) -> Paragraph<'a> {
    Paragraph::new(text).wrap(Wrap { trim: false })
}

/// `key description` pairs for footers and help.
pub fn hint_line<'a>(hints: &[(String, String)], theme: &Theme) -> Line<'a> {
    let mut spans = Vec::new();
    for (i, (key, what)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", theme.muted()));
        }
        spans.push(Span::styled(
            format!(" {key} "),
            Style::new()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        ));
        spans.push(Span::styled(format!(" {what}"), theme.muted()));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_rect_fits_inside() {
        let area = Rect::new(0, 0, 100, 40);
        assert_eq!(centered(area, 40, 10), Rect::new(30, 15, 40, 10));
        assert_eq!(centered(area, 200, 100), area, "clamped to the area");
    }

    #[test]
    fn qr_code_renders_square_ish_block() {
        let lines = qr_lines("bcrt1qexampleaddress");
        assert!(lines.len() > 10);
        let width = lines[0].chars().count();
        assert!(lines.iter().all(|l| l.chars().count() == width));
    }

    #[test]
    fn skeleton_has_requested_width() {
        let now = Instant::now();
        let line = skeleton(12, now, now, &Theme::default());
        assert_eq!(line.width(), 12);
    }
}
