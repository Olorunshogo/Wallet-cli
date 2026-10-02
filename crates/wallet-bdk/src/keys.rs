use bdk_wallet::KeychainKind;
use bdk_wallet::bitcoin::bip32::Xpriv;
use bdk_wallet::bitcoin::secp256k1::Secp256k1;
use bdk_wallet::bitcoin::{Network, NetworkKind};
use bdk_wallet::descriptor::{ExtendedDescriptor, IntoWalletDescriptor};
use bdk_wallet::keys::bip39::{Language, Mnemonic, WordCount};
use bdk_wallet::keys::{GeneratableKey, GeneratedKey};
use bdk_wallet::miniscript::Segwitv0;
use bdk_wallet::miniscript::descriptor::KeyMap;
use bdk_wallet::template::Bip84;
use wallet_core::{BoxError, KeySource, MnemonicLength};
use zeroize::Zeroizing;

/// Generate a new random English BIP39 mnemonic.
///
/// Entropy comes from the operating system's secure random number generator,
/// so every call returns a different phrase.
pub fn generate_mnemonic(length: MnemonicLength) -> Result<Zeroizing<String>, BoxError> {
    let words = match length {
        MnemonicLength::Words12 => WordCount::Words12,
        MnemonicLength::Words15 => WordCount::Words15,
        MnemonicLength::Words18 => WordCount::Words18,
        MnemonicLength::Words21 => WordCount::Words21,
        MnemonicLength::Words24 => WordCount::Words24,
    };
    let generated: GeneratedKey<Mnemonic, Segwitv0> =
        Mnemonic::generate((words, Language::English)).map_err(|e| {
            e.map(|e| e.to_string())
                .unwrap_or_else(|| "rng failure".into())
        })?;
    Ok(Zeroizing::new(generated.into_key().to_string()))
}

/// Check that `phrase` is a valid English BIP39 mnemonic: known words, a
/// supported length and a correct checksum. Use it to validate user input
/// before creating or restoring a wallet.
pub fn validate_mnemonic(phrase: &str) -> Result<(), BoxError> {
    Mnemonic::parse_in(Language::English, phrase)?;
    Ok(())
}

/// Public descriptors plus the private keys that go with them.
///
/// Only the public descriptors are handed to BDK. The key map stays in our
/// engine and is used for signing, so BDK never stores private keys.
pub(crate) struct Resolved {
    pub external: ExtendedDescriptor,
    pub internal: ExtendedDescriptor,
    pub keymap: KeyMap,
}

pub(crate) fn resolve(keys: &KeySource, network: Network) -> Result<Resolved, BoxError> {
    let secp = Secp256k1::new();
    let kind = NetworkKind::from(network);
    let ((external, mut keymap), (internal, internal_keys)) = match keys {
        KeySource::Mnemonic { phrase, passphrase } => {
            let mnemonic = Mnemonic::parse_in(Language::English, phrase.as_str())?;
            let passphrase = passphrase.as_ref().map(|p| p.as_str()).unwrap_or("");
            let seed = Zeroizing::new(mnemonic.to_seed(passphrase));
            let xprv = Xpriv::new_master(network, seed.as_slice())?;
            (
                Bip84(xprv, KeychainKind::External).into_wallet_descriptor(&secp, kind)?,
                Bip84(xprv, KeychainKind::Internal).into_wallet_descriptor(&secp, kind)?,
            )
        }
        KeySource::Descriptors { external, internal } => (
            external.as_str().into_wallet_descriptor(&secp, kind)?,
            internal.as_str().into_wallet_descriptor(&secp, kind)?,
        ),
    };
    keymap.extend(internal_keys);
    Ok(Resolved {
        external,
        internal,
        keymap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_length_generates_a_valid_mnemonic_of_that_size() {
        for length in MnemonicLength::ALL {
            let phrase = generate_mnemonic(length).unwrap();
            assert_eq!(phrase.split_whitespace().count(), length.words());
            validate_mnemonic(&phrase).unwrap();
        }
    }

    #[test]
    fn every_call_generates_a_different_mnemonic() {
        let a = generate_mnemonic(MnemonicLength::Words12).unwrap();
        let b = generate_mnemonic(MnemonicLength::Words12).unwrap();
        assert_ne!(*a, *b);
    }

    #[test]
    fn validate_mnemonic_rejects_bad_input() {
        let good = generate_mnemonic(MnemonicLength::Words12).unwrap();
        let words: Vec<&str> = good.split(' ').collect();

        // Wrong length.
        assert!(validate_mnemonic(&words[..11].join(" ")).is_err());
        // Not a BIP39 word.
        let mut bad = words.clone();
        bad[0] = "notaword";
        assert!(validate_mnemonic(&bad.join(" ")).is_err());
        // Every word valid, checksum wrong.
        assert!(validate_mnemonic(&["abandon"; 12].join(" ")).is_err());
        assert!(validate_mnemonic("").is_err());
    }

    #[test]
    fn invalid_mnemonic_is_rejected() {
        let keys = KeySource::mnemonic("not a real mnemonic phrase at all");
        assert!(resolve(&keys, Network::Regtest).is_err());
    }
}
