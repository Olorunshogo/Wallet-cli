use core::fmt;

use zeroize::Zeroizing;

/// Where a wallet's keys come from.
///
/// Secrets are held in [`Zeroizing`] buffers so they are wiped on drop, and
/// the `Debug` impl never prints them.
#[derive(Clone)]
pub enum KeySource {
    /// A BIP39 mnemonic. Engines derive descriptors from it according to
    /// [`AddressKind`] (default BIP84, native SegWit): e.g. for BIP84,
    /// `m/84'/coin'/0'/0/*` for receive and `.../1/*` for change.
    Mnemonic {
        /// The space-separated English words.
        phrase: Zeroizing<String>,
        /// Optional BIP39 passphrase ("25th word").
        passphrase: Option<Zeroizing<String>>,
    },
    /// Explicit output descriptors. Public (`xpub`/`tpub`) descriptors give a
    /// watch-only wallet; private (`xprv`/`tprv`) ones allow signing.
    Descriptors {
        /// Receive descriptor, ending in `/0/*` or similar.
        external: Zeroizing<String>,
        /// Change descriptor, ending in `/1/*` or similar.
        internal: Zeroizing<String>,
    },
}

impl KeySource {
    /// A mnemonic without a passphrase.
    pub fn mnemonic(phrase: impl Into<String>) -> Self {
        KeySource::Mnemonic {
            phrase: Zeroizing::new(phrase.into()),
            passphrase: None,
        }
    }

    /// A mnemonic with a BIP39 passphrase. A different passphrase gives a
    /// completely different wallet.
    pub fn mnemonic_with_passphrase(
        phrase: impl Into<String>,
        passphrase: impl Into<String>,
    ) -> Self {
        KeySource::Mnemonic {
            phrase: Zeroizing::new(phrase.into()),
            passphrase: Some(Zeroizing::new(passphrase.into())),
        }
    }

    /// A receive and a change descriptor.
    pub fn descriptors(external: impl Into<String>, internal: impl Into<String>) -> Self {
        KeySource::Descriptors {
            external: Zeroizing::new(external.into()),
            internal: Zeroizing::new(internal.into()),
        }
    }
}

/// Script type for a wallet's own addresses, derived from a mnemonic.
///
/// Only affects [`KeySource::Mnemonic`]: [`KeySource::Descriptors`] is
/// already explicit about its script type. Sending is unaffected either
/// way; a wallet can pay any address type regardless of its own kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum AddressKind {
    /// BIP44: `pkh(...)`, legacy P2PKH addresses (`1...`).
    Legacy,
    /// BIP49: `sh(wpkh(...))`, nested SegWit addresses (`3...`).
    NestedSegwit,
    /// BIP84: `wpkh(...)`, native SegWit addresses (`bc1q...`).
    #[default]
    Segwit,
    /// BIP86: `tr(...)`, Taproot addresses (`bc1p...`).
    Taproot,
}

impl AddressKind {
    /// Every kind, in the order a UI should offer them.
    pub const ALL: [AddressKind; 4] = [
        AddressKind::Legacy,
        AddressKind::NestedSegwit,
        AddressKind::Segwit,
        AddressKind::Taproot,
    ];

    /// Short, lowercase, hyphenated name: `legacy`, `nested-segwit`,
    /// `segwit`, `taproot`.
    pub fn as_str(self) -> &'static str {
        match self {
            AddressKind::Legacy => "legacy",
            AddressKind::NestedSegwit => "nested-segwit",
            AddressKind::Segwit => "segwit",
            AddressKind::Taproot => "taproot",
        }
    }
}

impl fmt::Display for AddressKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Length of a newly generated BIP39 mnemonic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MnemonicLength {
    /// 128 bits of entropy.
    #[default]
    Words12,
    /// 160 bits of entropy.
    Words15,
    /// 192 bits of entropy.
    Words18,
    /// 224 bits of entropy.
    Words21,
    /// 256 bits of entropy.
    Words24,
}

impl MnemonicLength {
    /// Every length BIP39 defines, shortest first.
    pub const ALL: [MnemonicLength; 5] = [
        MnemonicLength::Words12,
        MnemonicLength::Words15,
        MnemonicLength::Words18,
        MnemonicLength::Words21,
        MnemonicLength::Words24,
    ];

    /// Number of words.
    pub fn words(self) -> usize {
        match self {
            MnemonicLength::Words12 => 12,
            MnemonicLength::Words15 => 15,
            MnemonicLength::Words18 => 18,
            MnemonicLength::Words21 => 21,
            MnemonicLength::Words24 => 24,
        }
    }

    /// The length with this many words, if BIP39 defines one.
    pub fn from_words(words: usize) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.words() == words)
    }
}

impl fmt::Debug for KeySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeySource::Mnemonic { passphrase, .. } => f
                .debug_struct("Mnemonic")
                .field("phrase", &"<redacted>")
                .field("passphrase", &passphrase.as_ref().map(|_| "<redacted>"))
                .finish(),
            KeySource::Descriptors { .. } => f
                .debug_struct("Descriptors")
                .field("external", &"<redacted>")
                .field("internal", &"<redacted>")
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonic_lengths_round_trip_through_word_counts() {
        for length in MnemonicLength::ALL {
            assert_eq!(MnemonicLength::from_words(length.words()), Some(length));
        }
        assert_eq!(MnemonicLength::from_words(13), None);
    }

    #[test]
    fn debug_output_never_contains_secrets() {
        let keys = KeySource::mnemonic_with_passphrase("secret words here", "hunter2");
        let printed = format!("{keys:?}");
        assert!(!printed.contains("secret"));
        assert!(!printed.contains("hunter2"));
    }

    #[test]
    fn address_kind_defaults_to_segwit() {
        assert_eq!(AddressKind::default(), AddressKind::Segwit);
    }

    #[test]
    fn address_kind_names_are_distinct() {
        let names: Vec<&str> = AddressKind::ALL.iter().map(|k| k.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names.len(), sorted.len());
    }
}
