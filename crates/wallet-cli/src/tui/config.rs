//! Every tunable TUI value in one place, with defaults.
//!
//! Loaded from `<datadir>/tui.toml` when the file exists (or `--tui-config`).
//! Unknown keys are rejected so typos surface instead of being ignored.
//!
//! ```toml
//! refresh_secs = 5
//! fallback_fee_rate = 3
//!
//! [theme]
//! accent = "#f7931a"
//! ```

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use wallet::MnemonicLength;

use super::theme::ThemeConfig;

/// User-tunable settings. See the field docs for units.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TuiConfig {
    /// Seconds between automatic syncs. `0` turns auto-sync off.
    pub refresh_secs: u64,
    /// Seconds between attempts to reach the node while offline.
    pub reconnect_secs: u64,
    /// Confirmation target, in blocks, for fee estimates.
    pub fee_target_blocks: u16,
    /// Fee rate in sat/vB used when the node has no estimate yet.
    pub fallback_fee_rate: u64,
    /// Highest fee rate in sat/vB the forms accept, as a typo guard.
    pub max_fee_rate: u64,
    /// Redraws per second. Higher is smoother and uses more CPU.
    pub fps: u16,
    /// Seconds a notification stays on screen.
    pub toast_secs: u64,
    /// Milliseconds a balance takes to count up to its new value.
    pub tween_ms: u64,
    /// Number of words for newly generated mnemonics (12, 15, 18, 21, 24).
    pub mnemonic_words: usize,
    /// How many recent transactions the dashboard lists.
    pub recent_txs: usize,
    /// Colors. See [`ThemeConfig`].
    pub theme: ThemeConfig,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            refresh_secs: 10,
            reconnect_secs: 5,
            fee_target_blocks: 6,
            fallback_fee_rate: 2,
            max_fee_rate: 1_000,
            fps: 30,
            toast_secs: 4,
            tween_ms: 700,
            mnemonic_words: 12,
            recent_txs: 6,
            theme: ThemeConfig::default(),
        }
    }
}

impl TuiConfig {
    /// Load from `path`, or the defaults when the file does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid TUI config in {}", path.display()))
    }

    /// Parse and validate TOML text.
    pub fn parse(text: &str) -> Result<Self> {
        let config: TuiConfig = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    /// Reject values that would break the interface.
    pub fn validate(&self) -> Result<()> {
        if !(1..=120).contains(&self.fps) {
            bail!("fps must be between 1 and 120, got {}", self.fps);
        }
        if self.fallback_fee_rate == 0 || self.fallback_fee_rate > self.max_fee_rate {
            bail!(
                "fallback_fee_rate must be between 1 and max_fee_rate ({})",
                self.max_fee_rate
            );
        }
        if MnemonicLength::from_words(self.mnemonic_words).is_none() {
            bail!("mnemonic_words must be 12, 15, 18, 21 or 24");
        }
        if self.reconnect_secs == 0 {
            bail!("reconnect_secs must be at least 1");
        }
        if self.fee_target_blocks == 0 {
            bail!("fee_target_blocks must be at least 1");
        }
        self.theme.build().map_err(anyhow::Error::msg)?;
        Ok(())
    }

    /// Time between redraws.
    pub fn frame(&self) -> Duration {
        Duration::from_millis(1_000 / u64::from(self.fps))
    }

    /// Auto-sync interval, or `None` when disabled.
    pub fn refresh(&self) -> Option<Duration> {
        (self.refresh_secs > 0).then(|| Duration::from_secs(self.refresh_secs))
    }

    /// Time between reconnect attempts while offline.
    pub fn reconnect(&self) -> Duration {
        Duration::from_secs(self.reconnect_secs)
    }

    /// Mnemonic length for new wallets.
    pub fn mnemonic_length(&self) -> MnemonicLength {
        MnemonicLength::from_words(self.mnemonic_words).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        TuiConfig::default().validate().unwrap();
    }

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(TuiConfig::parse("").unwrap(), TuiConfig::default());
    }

    #[test]
    fn values_and_theme_can_be_overridden() {
        let config = TuiConfig::parse(
            r##"
            refresh_secs = 0
            mnemonic_words = 24
            [theme]
            accent = "#ff0000"
            "##,
        )
        .unwrap();
        assert_eq!(config.refresh(), None);
        assert_eq!(config.mnemonic_length(), MnemonicLength::Words24);
        assert_eq!(config.theme.accent, "#ff0000");
    }

    #[test]
    fn typos_and_bad_values_are_rejected() {
        assert!(TuiConfig::parse("refresh_sec = 5").is_err(), "unknown key");
        assert!(TuiConfig::parse("fps = 0").is_err());
        assert!(TuiConfig::parse("mnemonic_words = 13").is_err());
        assert!(TuiConfig::parse("fallback_fee_rate = 5000").is_err());
        assert!(TuiConfig::parse("[theme]\naccent = \"not-a-color\"").is_err());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let path = Path::new("/definitely/not/here/tui.toml");
        assert_eq!(TuiConfig::load(path).unwrap(), TuiConfig::default());
    }
}
