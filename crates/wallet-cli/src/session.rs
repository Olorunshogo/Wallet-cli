//! Creating, unlocking and opening wallets in a data directory.
//!
//! Shared by the CLI commands and the TUI so both store and unlock keys the
//! same way. Nothing here prompts the user; callers collect passwords in
//! whatever way suits their interface.

use anyhow::{Context, Result, bail};
use wallet::bitcoin::Network;
use wallet::{KeySource, Wallet};
use zeroize::Zeroizing;

use crate::config::DataDir;

/// Everything needed to create a wallet.
pub struct NewWallet {
    /// The wallet's keys. Must be a mnemonic so it can be stored.
    pub keys: KeySource,
    /// First block to scan on the initial sync.
    pub birthday: Option<u32>,
    /// Encrypt the stored mnemonic with this password. `None` stores it in
    /// plain text, readable only by the owner.
    pub password: Option<Zeroizing<String>>,
}

/// How the keys of an existing wallet are stored.
pub enum StoredKeys {
    /// Keys are available without a password.
    Plain(KeySource),
    /// Keys are encrypted; call [`unlock`] with the password.
    Encrypted,
    /// No keys are stored; the wallet opens watch-only.
    None,
}

/// Whether the data directory already holds a wallet.
pub fn exists(dir: &DataDir) -> bool {
    dir.db().exists()
}

/// Create a wallet and store its keys.
pub fn create(dir: &DataDir, network: Network, new: NewWallet) -> Result<Wallet> {
    if exists(dir) {
        bail!("a wallet already exists at {}", dir.path().display());
    }
    // Check before touching disk so a bad request leaves nothing behind.
    let KeySource::Mnemonic { phrase, passphrase } = &new.keys else {
        bail!("only mnemonic wallets can be stored by this client");
    };
    let phrase = phrase.clone();
    let passphrase = passphrase.clone();
    dir.ensure()?;

    let mut builder = Wallet::builder(network).keys(new.keys).database(dir.db());
    if let Some(height) = new.birthday {
        builder = builder.birthday(height);
    }
    let wallet = builder.create().context("could not create wallet")?;
    dir.save_meta(new.birthday)?;

    match &new.password {
        Some(password) => {
            dir.save_mnemonic_encrypted(&phrase, passphrase.as_ref().map(|p| p.as_str()), password)?
        }
        None => {
            dir.save_mnemonic(&phrase)?;
            if let Some(p) = &passphrase {
                dir.save_passphrase(p)?;
            }
        }
    }
    return Ok(wallet);
}

/// How the existing wallet's keys are stored.
pub fn stored_keys(dir: &DataDir) -> Result<StoredKeys> {
    if dir.is_encrypted() {
        return Ok(StoredKeys::Encrypted);
    }
    return Ok(match dir.load_keys()? {
        Some(keys) => StoredKeys::Plain(keys),
        None => StoredKeys::None,
    });
}

/// Decrypt the stored keys.
pub fn unlock(dir: &DataDir, password: &str) -> Result<KeySource> {
    dir.load_mnemonic_encrypted(password)
}

/// Open the existing wallet, watch-only when `keys` is `None`.
pub fn open(dir: &DataDir, network: Network, keys: Option<KeySource>) -> Result<Wallet> {
    if !exists(dir) {
        bail!(
            "no wallet at {}; run `init` or `restore` first",
            dir.path().display()
        );
    }
    let mut builder = Wallet::builder(network).database(dir.db());
    if let Some(keys) = keys {
        builder = builder.keys(keys);
    }
    if let Some(height) = dir.load_meta()?.birthday {
        builder = builder.birthday(height);
    }
    builder.load().context("could not load wallet")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_phrase() -> String {
        wallet::generate_mnemonic(wallet::MnemonicLength::Words12)
            .unwrap()
            .to_string()
    }

    fn datadir() -> (tempfile::TempDir, DataDir) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = DataDir::at(tmp.path().join("w"));
        (tmp, dir)
    }

    fn reopen(dir: &DataDir, keys: Option<KeySource>) -> Wallet {
        open(dir, Network::Regtest, keys).unwrap()
    }

    #[test]
    fn plain_wallet_reopens_with_signing_keys() {
        let (_tmp, dir) = datadir();
        let new = NewWallet {
            keys: KeySource::mnemonic(fresh_phrase()),
            birthday: Some(7),
            password: None,
        };
        let address = create(&dir, Network::Regtest, new)
            .unwrap()
            .new_address()
            .unwrap();
        let StoredKeys::Plain(keys) = stored_keys(&dir).unwrap() else {
            panic!("expected plain keys");
        };
        let mut wallet = reopen(&dir, Some(keys));
        assert!(wallet.can_sign());
        assert_eq!(wallet.new_address().unwrap(), address);
        assert_eq!(dir.load_meta().unwrap().birthday, Some(7));
    }

    #[test]
    fn encrypted_wallet_with_passphrase_unlocks_to_the_same_wallet() {
        // Regression: the passphrase used to be dropped when encrypting, so
        // the reopened wallet derived different keys.
        let (_tmp, dir) = datadir();
        let new = NewWallet {
            keys: KeySource::mnemonic_with_passphrase(fresh_phrase(), "extra words"),
            birthday: None,
            password: Some(Zeroizing::new("pw".into())),
        };
        let address = create(&dir, Network::Regtest, new)
            .unwrap()
            .new_address()
            .unwrap();
        assert!(!dir.path().join("mnemonic").exists(), "no plain-text copy");
        assert!(matches!(stored_keys(&dir).unwrap(), StoredKeys::Encrypted));

        let keys = unlock(&dir, "pw").unwrap();
        let mut wallet = reopen(&dir, Some(keys));
        assert!(wallet.can_sign());
        assert_eq!(wallet.new_address().unwrap(), address);
        assert!(unlock(&dir, "wrong").is_err());
    }

    #[test]
    fn opening_without_keys_is_watch_only() {
        let (_tmp, dir) = datadir();
        let new = NewWallet {
            keys: KeySource::mnemonic(fresh_phrase()),
            birthday: None,
            password: Some(Zeroizing::new("pw".into())),
        };
        create(&dir, Network::Regtest, new).unwrap();
        assert!(!reopen(&dir, None).can_sign());
    }

    #[test]
    fn creating_twice_or_opening_nothing_fails_cleanly() {
        let (_tmp, dir) = datadir();
        assert!(open(&dir, Network::Regtest, None).is_err());
        let new = || NewWallet {
            keys: KeySource::mnemonic(fresh_phrase()),
            birthday: None,
            password: None,
        };
        create(&dir, Network::Regtest, new()).unwrap();
        let err = create(&dir, Network::Regtest, new()).err().unwrap();
        assert!(err.to_string().contains("already exists"));
    }
}
