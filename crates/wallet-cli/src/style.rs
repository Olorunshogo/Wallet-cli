//! Building blocks for the CLI's printed output, colored from [`PALETTE`].
//!
//! Colors are turned off globally (see [`init`]) when output is not a
//! terminal, with `--json`, or when `NO_COLOR` is set, so scripts get plain
//! text with no escape codes.

use colored::{ColoredString, Colorize};

use crate::palette::{PALETTE, Tint};

/// Width of rules and wrapped blocks.
pub const WIDTH: usize = 72;

/// Decide once whether printed output may use color.
pub fn init(json: bool) {
    use std::io::IsTerminal;
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    let enabled = !json && !no_color && std::io::stdout().is_terminal();
    colored::control::set_override(enabled);
}

fn tint(text: &str, color: Tint) -> ColoredString {
    text.color(color.term())
}

/// ` TEXT ` in dark text on a colored background.
pub fn badge(text: &str, background: Tint) -> ColoredString {
    format!(" {text} ")
        .color(PALETTE.on_badge.term())
        .on_color(background.term())
        .bold()
}

/// A field name that should recede.
pub fn label(text: &str) -> ColoredString {
    tint(text, PALETTE.label)
}

/// A value the user reads.
pub fn value(text: &str) -> ColoredString {
    tint(text, PALETTE.value)
}

/// A Bitcoin amount.
pub fn amount(text: &str) -> ColoredString {
    tint(text, PALETTE.amount).bold()
}

/// Success text.
pub fn success(text: &str) -> ColoredString {
    tint(text, PALETTE.success)
}

/// Warning text.
pub fn warning(text: &str) -> ColoredString {
    tint(text, PALETTE.warning)
}

/// Error text.
pub fn error(text: &str) -> ColoredString {
    tint(text, PALETTE.error)
}

/// Informational text.
pub fn info(text: &str) -> ColoredString {
    tint(text, PALETTE.info)
}

/// A bold title in the brand color.
pub fn title(text: &str) -> ColoredString {
    tint(text, PALETTE.brand).bold()
}

/// A horizontal rule: heavy (`═`) under headers, light (`─`) between items.
pub fn rule(heavy: bool) -> ColoredString {
    tint(&if heavy { "═" } else { "─" }.repeat(WIDTH), PALETTE.border)
}

/// `  Label         value` with the label padded to `width`.
pub fn row(name: &str, width: usize, val: impl std::fmt::Display) -> String {
    format!("  {}  {val}", label(&format!("{name:<width$}")))
}
