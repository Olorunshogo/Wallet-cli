//! Every color the CLI and the TUI use, declared in one place.
//!
//! The look: a very dark background, cyan borders and rules, and bitcoin
//! orange for the brand, the active tab and amounts. Change a role here and
//! both the printed CLI output and the TUI follow; the TUI can still override
//! any of them in `tui.toml`.

/// An exact RGB color. Exact values keep the look the same on every
/// terminal theme (named colors like "yellow" vary between terminals).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tint(pub u8, pub u8, pub u8);

impl Tint {
    /// The name `tui.toml` and ratatui understand, e.g. `"#f7931a"`.
    pub fn name(self) -> String {
        let Tint(r, g, b) = self;
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    /// The same color for printed terminal output (24-bit color).
    pub fn term(self) -> colored::Color {
        let Tint(r, g, b) = self;
        colored::Color::TrueColor { r, g, b }
    }
}

/// Colors by role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Behind everything in the TUI.
    pub background: Tint,
    /// Borders, rules and dividers.
    pub border: Tint,
    /// Titles, the app name, the active tab, focused borders.
    pub brand: Tint,
    /// Field labels and hints: everything that should recede.
    pub label: Tint,
    /// Values the user reads: addresses, txids, numbers.
    pub value: Tint,
    /// Bitcoin amounts.
    pub amount: Tint,
    /// Something worked, confirmed transactions.
    pub success: Tint,
    /// Needs attention, unconfirmed transactions.
    pub warning: Tint,
    /// Something failed.
    pub error: Tint,
    /// Neutral information, immature coins.
    pub info: Tint,
    /// Text drawn on top of a colored badge or the active tab.
    pub on_badge: Tint,
    /// Background of the selected row in lists.
    pub selection: Tint,
    /// Loading placeholders and their moving highlight.
    pub skeleton: Tint,
    #[allow(missing_docs)]
    pub shimmer: Tint,
    /// Network badges.
    pub mainnet: Tint,
    #[allow(missing_docs)]
    pub testnet: Tint,
    #[allow(missing_docs)]
    pub signet: Tint,
    #[allow(missing_docs)]
    pub regtest: Tint,
}

/// Bitcoin orange.
pub const BITCOIN_ORANGE: Tint = Tint(0xf7, 0x93, 0x1a);

/// The palette in use.
pub const PALETTE: Palette = Palette {
    background: Tint(0x0a, 0x0e, 0x14),
    border: Tint(0x22, 0xb8, 0xcf),
    brand: BITCOIN_ORANGE,
    label: Tint(0x7d, 0x8a, 0x99),
    value: Tint(0xe6, 0xed, 0xf3),
    amount: BITCOIN_ORANGE,
    success: Tint(0x3f, 0xb9, 0x50),
    warning: Tint(0xe3, 0xb3, 0x41),
    error: Tint(0xf8, 0x51, 0x49),
    info: Tint(0x22, 0xb8, 0xcf),
    on_badge: Tint(0x0a, 0x0e, 0x14),
    selection: Tint(0x1a, 0x26, 0x33),
    skeleton: Tint(0x1a, 0x22, 0x2c),
    shimmer: Tint(0x2e, 0x3b, 0x4a),
    mainnet: Tint(0xf8, 0x51, 0x49),
    testnet: Tint(0x3f, 0xb9, 0x50),
    signet: Tint(0xbc, 0x8c, 0xff),
    regtest: Tint(0x22, 0xb8, 0xcf),
};

impl Palette {
    /// Badge color for a network.
    pub fn network(&self, network: wallet::bitcoin::Network) -> Tint {
        use wallet::bitcoin::Network;
        match network {
            Network::Bitcoin => self.mainnet,
            Network::Signet => self.signet,
            Network::Regtest => self.regtest,
            _ => self.testnet,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_palette_color_is_understood_by_the_tui() {
        let p = PALETTE;
        for tint in [
            p.background,
            p.border,
            p.brand,
            p.label,
            p.value,
            p.amount,
            p.success,
            p.warning,
            p.error,
            p.info,
            p.on_badge,
            p.selection,
            p.skeleton,
            p.shimmer,
            p.mainnet,
            p.testnet,
            p.signet,
            p.regtest,
        ] {
            assert!(
                tint.name().parse::<ratatui::style::Color>().is_ok(),
                "{}",
                tint.name()
            );
        }
        assert_eq!(BITCOIN_ORANGE.name(), "#f7931a");
    }
}
