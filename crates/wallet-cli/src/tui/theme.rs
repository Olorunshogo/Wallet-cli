//! Colors as named variables.
//!
//! [`ThemeConfig`] holds color strings (names like `"green"` or hex like
//! `"#f7931a"`) so they can be overridden from `tui.toml`. [`Theme`] is the
//! parsed form the widgets use, plus style helpers so no widget picks a raw
//! color itself.

use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;
use wallet::TxStatus;

use crate::palette::{PALETTE, Tint};
use wallet::bitcoin::Network;

/// Color settings as written in `tui.toml`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[allow(missing_docs)]
pub struct ThemeConfig {
    pub background: String,
    pub accent: String,
    pub text: String,
    pub muted: String,
    pub border: String,
    pub border_focus: String,
    pub selection: String,
    pub success: String,
    pub warning: String,
    pub error: String,
    pub info: String,
    pub confirmed: String,
    pub unconfirmed: String,
    pub immature: String,
    pub skeleton: String,
    pub skeleton_shine: String,
    pub mainnet: String,
    pub testnet: String,
    pub signet: String,
    pub regtest: String,
}

impl Default for ThemeConfig {
    /// Every default comes from the shared palette (`palette.rs`), so the TUI
    /// and the printed CLI output always match.
    fn default() -> Self {
        let p = PALETTE;
        let s = |t: Tint| t.name();
        Self {
            background: s(p.background),
            accent: s(p.brand),
            text: s(p.value),
            muted: s(p.label),
            border: s(p.border),
            border_focus: s(p.brand),
            selection: s(p.selection),
            success: s(p.success),
            warning: s(p.warning),
            error: s(p.error),
            info: s(p.info),
            confirmed: s(p.success),
            unconfirmed: s(p.warning),
            immature: s(p.info),
            skeleton: s(p.skeleton),
            skeleton_shine: s(p.shimmer),
            mainnet: s(p.mainnet),
            testnet: s(p.testnet),
            signet: s(p.signet),
            regtest: s(p.regtest),
        }
    }
}

impl ThemeConfig {
    /// Parse every color, naming the first invalid one.
    pub fn build(&self) -> Result<Theme, String> {
        let c = |name: &str, value: &str| -> Result<Color, String> {
            value
                .parse::<Color>()
                .map_err(|_| format!("theme.{name}: \"{value}\" is not a color"))
        };
        Ok(Theme {
            background: c("background", &self.background)?,
            accent: c("accent", &self.accent)?,
            text: c("text", &self.text)?,
            muted: c("muted", &self.muted)?,
            border: c("border", &self.border)?,
            border_focus: c("border_focus", &self.border_focus)?,
            selection: c("selection", &self.selection)?,
            success: c("success", &self.success)?,
            warning: c("warning", &self.warning)?,
            error: c("error", &self.error)?,
            info: c("info", &self.info)?,
            confirmed: c("confirmed", &self.confirmed)?,
            unconfirmed: c("unconfirmed", &self.unconfirmed)?,
            immature: c("immature", &self.immature)?,
            skeleton: c("skeleton", &self.skeleton)?,
            skeleton_shine: c("skeleton_shine", &self.skeleton_shine)?,
            mainnet: c("mainnet", &self.mainnet)?,
            testnet: c("testnet", &self.testnet)?,
            signet: c("signet", &self.signet)?,
            regtest: c("regtest", &self.regtest)?,
        })
    }
}

/// Parsed colors. Widgets go through the helpers below.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(missing_docs)]
pub struct Theme {
    pub background: Color,
    pub accent: Color,
    pub text: Color,
    pub muted: Color,
    pub border: Color,
    pub border_focus: Color,
    pub selection: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub info: Color,
    pub confirmed: Color,
    pub unconfirmed: Color,
    pub immature: Color,
    pub skeleton: Color,
    pub skeleton_shine: Color,
    pub mainnet: Color,
    pub testnet: Color,
    pub signet: Color,
    pub regtest: Color,
}

impl Default for Theme {
    fn default() -> Self {
        ThemeConfig::default()
            .build()
            .expect("default theme is valid")
    }
}

/// Kinds of message, each with its own color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Something worked.
    Success,
    /// Neutral information.
    Info,
    /// Needs attention but nothing failed.
    Warning,
    /// Something failed.
    Error,
}

impl Theme {
    /// The whole screen: body text on the dark background.
    pub fn base(&self) -> Style {
        Style::new().fg(self.text).bg(self.background)
    }

    /// The active tab: dark text on the brand color.
    pub fn active_tab(&self) -> Style {
        Style::new()
            .fg(self.background)
            .bg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    /// Body text.
    pub fn text(&self) -> Style {
        Style::new().fg(self.text)
    }

    /// Secondary text: labels, hints, timestamps.
    pub fn muted(&self) -> Style {
        Style::new().fg(self.muted)
    }

    /// Titles and emphasis.
    pub fn title(&self) -> Style {
        Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
    }

    /// Panel borders, highlighted when focused.
    pub fn border(&self, focused: bool) -> Style {
        Style::new().fg(if focused {
            self.border_focus
        } else {
            self.border
        })
    }

    /// The selected row in lists and tables.
    pub fn selected(&self) -> Style {
        Style::new().bg(self.selection).add_modifier(Modifier::BOLD)
    }

    /// Color for a message tone.
    pub fn tone(&self, tone: Tone) -> Color {
        match tone {
            Tone::Success => self.success,
            Tone::Info => self.info,
            Tone::Warning => self.warning,
            Tone::Error => self.error,
        }
    }

    /// Color for a transaction status.
    pub fn status(&self, status: &TxStatus) -> Style {
        Style::new().fg(match status {
            TxStatus::Confirmed { .. } => self.confirmed,
            TxStatus::Unconfirmed => self.unconfirmed,
        })
    }

    /// Badge style for a network; mainnet stands out on purpose.
    pub fn network(&self, network: Network) -> Style {
        let color = match network {
            Network::Bitcoin => self.mainnet,
            Network::Signet => self.signet,
            Network::Regtest => self.regtest,
            _ => self.testnet,
        };
        Style::new()
            .fg(Color::Black)
            .bg(color)
            .add_modifier(Modifier::BOLD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_parses() {
        let theme = Theme::default();
        assert_eq!(theme.accent, Color::Rgb(0xf7, 0x93, 0x1a), "bitcoin orange");
        assert_eq!(theme.border, Color::Rgb(0x22, 0xb8, 0xcf), "cyan borders");
        assert_eq!(
            theme.base().bg,
            Some(Color::Rgb(0x0a, 0x0e, 0x14)),
            "dark background"
        );
    }

    #[test]
    fn bad_color_names_the_field() {
        let config = ThemeConfig {
            warning: "orangeish".into(),
            ..ThemeConfig::default()
        };
        let err = config.build().unwrap_err();
        assert!(err.contains("theme.warning"), "{err}");
    }

    #[test]
    fn styles_follow_the_theme() {
        let theme = Theme::default();
        let confirmed = TxStatus::Confirmed {
            height: 1,
            confirmations: 1,
        };
        assert_eq!(theme.status(&confirmed).fg, Some(theme.confirmed));
        assert_eq!(
            theme.status(&TxStatus::Unconfirmed).fg,
            Some(theme.unconfirmed)
        );
        assert_eq!(theme.network(Network::Bitcoin).bg, Some(theme.mainnet));
        assert_eq!(theme.border(true).fg, Some(theme.border_focus));
    }
}
