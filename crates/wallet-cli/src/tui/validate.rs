//! Reusable input validators.
//!
//! Each validator turns raw text into a typed value or a short message the
//! form shows under the field. They know nothing about the UI, so any form
//! (or the CLI) can reuse them.

use wallet::bitcoin::address::NetworkUnchecked;
use wallet::bitcoin::{Address, Amount, Denomination, FeeRate, Network};

/// Parse and check one kind of input.
pub trait Validator {
    /// What valid input becomes.
    type Output;

    /// Turn `input` into a value, or explain what is wrong in a few words.
    fn validate(&self, input: &str) -> Result<Self::Output, String>;
}

// === Bitcoin values

/// An address valid for one network.
#[derive(Debug, Clone, Copy)]
pub struct AddressValidator {
    /// The wallet's network.
    pub network: Network,
}

impl Validator for AddressValidator {
    type Output = Address<NetworkUnchecked>;

    fn validate(&self, input: &str) -> Result<Self::Output, String> {
        let input = input.trim();
        if input.is_empty() {
            return Err("enter an address".into());
        }
        let address: Address<NetworkUnchecked> = input
            .parse()
            .map_err(|_| "not a valid bitcoin address".to_string())?;
        if !address.is_valid_for_network(self.network) {
            return Err(format!("not a {} address", self.network));
        }
        Ok(address)
    }
}

/// An amount in sats (`50000`, `50,000`, `50000 sat`) or BTC (`0.0005 btc`),
/// optionally capped.
#[derive(Debug, Clone, Copy, Default)]
pub struct AmountValidator {
    /// Largest acceptable amount, e.g. the spendable balance.
    pub max: Option<Amount>,
}

impl Validator for AmountValidator {
    type Output = Amount;

    fn validate(&self, input: &str) -> Result<Self::Output, String> {
        let text = input.trim().to_lowercase().replace([',', '_'], "");
        if text.is_empty() {
            return Err("enter an amount".into());
        }
        let amount = if let Some(number) = text.strip_suffix("btc") {
            Amount::from_str_in(number.trim(), Denomination::Bitcoin)
                .map_err(|_| "not a valid BTC amount".to_string())?
        } else {
            let number = text
                .strip_suffix("sats")
                .or_else(|| text.strip_suffix("sat"))
                .unwrap_or(&text)
                .trim();
            let sats: u64 = number
                .parse()
                .map_err(|_| "use whole sats, or add \"btc\" for decimals".to_string())?;
            Amount::from_sat(sats)
        };
        if amount == Amount::ZERO {
            return Err("amount must be more than zero".into());
        }
        if let Some(max) = self.max
            && amount > max
        {
            return Err(format!(
                "more than the {} available",
                super::format::sats(max)
            ));
        }
        Ok(amount)
    }
}

/// A whole fee rate in sat/vB within bounds.
#[derive(Debug, Clone, Copy)]
pub struct FeeRateValidator {
    /// Lowest accepted rate.
    pub min: u64,
    /// Highest accepted rate, as a typo guard.
    pub max: u64,
}

impl Validator for FeeRateValidator {
    type Output = FeeRate;

    fn validate(&self, input: &str) -> Result<Self::Output, String> {
        let text = input.trim().to_lowercase();
        let number = text.strip_suffix("sat/vb").unwrap_or(&text).trim();
        if number.is_empty() {
            return Err("enter a fee rate".into());
        }
        let rate: u64 = number
            .parse()
            .map_err(|_| "whole sat/vB only".to_string())?;
        if rate < self.min || rate > self.max {
            return Err(format!("between {} and {} sat/vB", self.min, self.max));
        }
        FeeRate::from_sat_per_vb(rate).ok_or_else(|| "fee rate too large".into())
    }
}

// === Keys and secrets

/// A BIP39 mnemonic with a correct checksum. Extra spaces are normalised.
#[derive(Debug, Clone, Copy, Default)]
pub struct MnemonicValidator;

impl Validator for MnemonicValidator {
    type Output = String;

    fn validate(&self, input: &str) -> Result<Self::Output, String> {
        let normalised = input
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let words = normalised.split(' ').filter(|w| !w.is_empty()).count();
        if words == 0 {
            return Err("enter your recovery words".into());
        }
        if wallet::MnemonicLength::from_words(words).is_none() {
            return Err(format!("{words} words; expected 12, 15, 18, 21 or 24"));
        }
        wallet::validate_mnemonic(&normalised)
            .map_err(|_| "unknown word or wrong checksum".to_string())?;
        Ok(normalised)
    }
}

/// A password with a minimum length.
#[derive(Debug, Clone, Copy)]
pub struct PasswordValidator {
    /// Fewest characters accepted.
    pub min_len: usize,
}

impl Validator for PasswordValidator {
    type Output = String;

    fn validate(&self, input: &str) -> Result<Self::Output, String> {
        if input.chars().count() < self.min_len {
            return Err(format!("at least {} characters", self.min_len));
        }
        Ok(input.to_string())
    }
}

/// A new wallet name: valid as a directory name and not taken yet.
#[derive(Debug, Clone, Default)]
pub struct WalletNameValidator {
    /// Names already in use.
    pub taken: Vec<String>,
}

impl Validator for WalletNameValidator {
    type Output = String;

    fn validate(&self, input: &str) -> Result<Self::Output, String> {
        let name = input.trim().to_lowercase();
        crate::wallets::validate_name(&name)?;
        if self.taken.contains(&name) {
            return Err(format!("{name} already exists"));
        }
        Ok(name)
    }
}

/// Any text, including empty (for optional fields such as a passphrase).
#[derive(Debug, Clone, Copy, Default)]
pub struct AnyText;

impl Validator for AnyText {
    type Output = String;

    fn validate(&self, input: &str) -> Result<Self::Output, String> {
        Ok(input.to_string())
    }
}

/// An optional block height; empty means "not set".
#[derive(Debug, Clone, Copy, Default)]
pub struct OptionalHeightValidator;

impl Validator for OptionalHeightValidator {
    type Output = Option<u32>;

    fn validate(&self, input: &str) -> Result<Self::Output, String> {
        let text = input.trim().replace([',', '_'], "");
        if text.is_empty() {
            return Ok(None);
        }
        text.parse()
            .map(Some)
            .map_err(|_| "a block height, or leave empty".into())
    }
}

#[cfg(test)]
mod tests {
    use wallet::{KeySource, MnemonicLength, Wallet};

    use super::*;

    fn regtest_address() -> String {
        let phrase = wallet::generate_mnemonic(MnemonicLength::Words12).unwrap();
        let mut w = Wallet::builder(Network::Regtest)
            .keys(KeySource::mnemonic(phrase.as_str()))
            .create()
            .unwrap();
        w.new_address().unwrap().address.to_string()
    }

    #[test]
    fn addresses_are_checked_against_the_network() {
        let v = AddressValidator {
            network: Network::Regtest,
        };
        let address = regtest_address();
        assert!(v.validate(&format!("  {address} ")).is_ok());
        assert_eq!(v.validate("").unwrap_err(), "enter an address");
        assert_eq!(
            v.validate("hello").unwrap_err(),
            "not a valid bitcoin address"
        );
        let mainnet = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
        assert_eq!(v.validate(mainnet).unwrap_err(), "not a regtest address");
    }

    #[test]
    fn amounts_accept_sats_and_btc() {
        let v = AmountValidator::default();
        assert_eq!(v.validate("50000").unwrap(), Amount::from_sat(50_000));
        assert_eq!(v.validate("50,000").unwrap(), Amount::from_sat(50_000));
        assert_eq!(v.validate("50_000 sats").unwrap(), Amount::from_sat(50_000));
        assert_eq!(v.validate("0.0005 BTC").unwrap(), Amount::from_sat(50_000));
        assert_eq!(v.validate("1btc").unwrap(), Amount::from_sat(100_000_000));
    }

    #[test]
    fn bad_amounts_explain_themselves() {
        let v = AmountValidator {
            max: Some(Amount::from_sat(1_000)),
        };
        assert_eq!(v.validate("").unwrap_err(), "enter an amount");
        assert_eq!(
            v.validate("0").unwrap_err(),
            "amount must be more than zero"
        );
        assert!(v.validate("1.5").unwrap_err().contains("btc"));
        assert!(v.validate("abc btc").unwrap_err().contains("BTC"));
        assert!(
            v.validate("2000")
                .unwrap_err()
                .contains("1,000 sat available")
        );
    }

    #[test]
    fn fee_rates_are_bounded() {
        let v = FeeRateValidator { min: 1, max: 500 };
        assert_eq!(v.validate("5").unwrap(), FeeRate::from_sat_per_vb_u32(5));
        assert_eq!(
            v.validate("5 sat/vB").unwrap(),
            FeeRate::from_sat_per_vb_u32(5)
        );
        assert!(v.validate("0").is_err());
        assert!(v.validate("501").is_err());
        assert!(v.validate("2.5").is_err());
        assert!(v.validate("").is_err());
    }

    #[test]
    fn mnemonics_are_normalised_and_checked() {
        let phrase = wallet::generate_mnemonic(MnemonicLength::Words15).unwrap();
        let messy = format!("  {}  ", phrase.to_uppercase().replace(' ', "   "));
        assert_eq!(MnemonicValidator.validate(&messy).unwrap(), *phrase);

        let words: Vec<&str> = phrase.split(' ').collect();
        assert!(
            MnemonicValidator
                .validate(&words[..13].join(" "))
                .unwrap_err()
                .contains("13 words")
        );
        assert!(
            MnemonicValidator
                .validate(&["abandon"; 12].join(" "))
                .unwrap_err()
                .contains("checksum")
        );
        assert!(MnemonicValidator.validate("").is_err());
    }

    #[test]
    fn wallet_names_are_checked_and_unique() {
        let v = WalletNameValidator {
            taken: vec!["alice".into()],
        };
        assert_eq!(v.validate("  Bob ").unwrap(), "bob");
        assert!(v.validate("alice").unwrap_err().contains("exists"));
        assert!(v.validate("no spaces").is_err());
    }

    #[test]
    fn passwords_heights_and_free_text() {
        assert!(PasswordValidator { min_len: 8 }.validate("short").is_err());
        assert!(
            PasswordValidator { min_len: 8 }
                .validate("long enough")
                .is_ok()
        );
        assert_eq!(OptionalHeightValidator.validate("").unwrap(), None);
        assert_eq!(
            OptionalHeightValidator.validate("120,000").unwrap(),
            Some(120_000)
        );
        assert!(OptionalHeightValidator.validate("-1").is_err());
        assert_eq!(AnyText.validate("").unwrap(), "");
    }
}
