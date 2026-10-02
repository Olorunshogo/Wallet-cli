//! Named wallets: several wallets side by side, picked by name.
//!
//! ```text
//! .wallet/<network>/
//!   wallets/alice/   wallet.sqlite, recovery words, meta.json, outbox/
//!   wallets/bob/
//!   default          name of the wallet used when none is named
//!   node/            the shared local regtest node (see node.rs)
//! ```
//!
//! A wallet from before named wallets (files directly in `.wallet/<network>`)
//! is moved to `wallets/default` the first time.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use wallet::bitcoin::Network;

use crate::config::DataDir;
use crate::session;

/// Name used when nothing else is chosen.
pub const DEFAULT_NAME: &str = "default";

/// Files that belong to one wallet (moved together during migration).
const WALLET_FILES: [&str; 8] = [
    "wallet.sqlite",
    "mnemonic",
    "passphrase",
    "mnemonic.enc",
    "meta.json",
    "outbox",
    "tui.toml",
    "tui.log",
];

/// One wallet, as shown in lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Its name.
    pub name: String,
    /// Recovery words are password-protected.
    pub encrypted: bool,
    /// Balance in sats at the last sync, if it has synced.
    pub last_balance_sat: Option<u64>,
    /// Used when no wallet is named.
    pub is_default: bool,
}

/// The wallets of one network.
#[derive(Debug, Clone)]
pub struct Wallets {
    root: PathBuf,
}

/// Wallet names: short, lowercase, safe as a directory name.
pub fn validate_name(name: &str) -> Result<(), String> {
    let ok_len = (1..=32).contains(&name.len());
    let ok_chars = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !ok_len || !ok_chars || name.starts_with('-') {
        return Err("1 to 32 characters: a-z, 0-9, - and _".into());
    }
    Ok(())
}

impl Wallets {
    /// Wallets under `home/<network>` (default home: `.wallet`).
    pub fn new(home: Option<PathBuf>, network: Network) -> Self {
        let home = home.unwrap_or_else(|| PathBuf::from(".wallet"));
        Self {
            root: home.join(network.to_core_arg()),
        }
    }

    /// The network directory (`.wallet/regtest`).
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn wallets_dir(&self) -> PathBuf {
        self.root.join("wallets")
    }

    /// The directory of wallet `name` (it may not exist yet).
    pub fn dir(&self, name: &str) -> DataDir {
        DataDir::at(self.wallets_dir().join(name))
    }

    /// Whether a wallet called `name` exists.
    pub fn exists(&self, name: &str) -> bool {
        session::exists(&self.dir(name))
    }

    /// Wallet names, sorted.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.wallets_dir())
            .map(|dir| {
                dir.filter_map(|e| e.ok())
                    .filter(|e| e.path().join("wallet.sqlite").exists())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// The default wallet: the one set with `use`, else the only wallet,
    /// else `default`.
    pub fn default_name(&self) -> String {
        let pointer = fs::read_to_string(self.root.join("default"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|n| validate_name(n).is_ok() && self.exists(n));
        if let Some(name) = pointer {
            return name;
        }
        let names = self.names();
        match names.as_slice() {
            [only] => only.clone(),
            _ => DEFAULT_NAME.to_string(),
        }
    }

    /// `name` if given, else the default.
    pub fn resolve(&self, name: Option<&str>) -> Result<String> {
        match name {
            Some(n) => {
                validate_name(n).map_err(|e| anyhow::anyhow!("invalid wallet name {n:?}: {e}"))?;
                Ok(n.to_string())
            }
            None => Ok(self.default_name()),
        }
    }

    /// Make `name` the default.
    pub fn set_default(&self, name: &str) -> Result<()> {
        if !self.exists(name) {
            bail!("no wallet named {name:?}; see `wallets`");
        }
        fs::create_dir_all(&self.root)?;
        fs::write(self.root.join("default"), name).context("could not save the default wallet")
    }

    /// Every wallet with what lists show.
    pub fn list(&self) -> Vec<Entry> {
        let default = self.default_name();
        self.names()
            .into_iter()
            .map(|name| {
                let dir = self.dir(&name);
                Entry {
                    encrypted: dir.is_encrypted(),
                    last_balance_sat: dir.load_meta().ok().and_then(|m| m.last_balance_sat),
                    is_default: name == default,
                    name,
                }
            })
            .collect()
    }

    /// Rename a wallet (and keep it the default if it was).
    pub fn rename(&self, old: &str, new: &str) -> Result<()> {
        validate_name(new).map_err(|e| anyhow::anyhow!("invalid wallet name {new:?}: {e}"))?;
        if !self.exists(old) {
            bail!("no wallet named {old:?}");
        }
        if self.dir(new).path().exists() {
            bail!("a wallet named {new:?} already exists");
        }
        let was_default = self.default_name() == old;
        fs::rename(self.dir(old).path(), self.dir(new).path())
            .with_context(|| format!("could not rename {old} to {new}"))?;
        if was_default {
            self.set_default(new)?;
        }
        Ok(())
    }

    /// Delete a wallet's files. Its recovery words are the only way back.
    pub fn remove(&self, name: &str) -> Result<()> {
        if !self.exists(name) {
            bail!("no wallet named {name:?}");
        }
        let was_default = self.default_name() == name;
        fs::remove_dir_all(self.dir(name).path())
            .with_context(|| format!("could not remove {name}"))?;
        if was_default {
            let _ = fs::remove_file(self.root.join("default"));
        }
        Ok(())
    }

    /// Move a wallet from before named wallets into `wallets/default`.
    /// Returns a message when something was moved.
    pub fn migrate_legacy(&self) -> Result<Option<String>> {
        if !self.root.join("wallet.sqlite").exists() {
            return Ok(None);
        }
        let target = self.dir(DEFAULT_NAME);
        if target.path().join("wallet.sqlite").exists() {
            bail!(
                "found an old wallet in {} and a wallet named \"default\"; move one of them by hand",
                self.root.display()
            );
        }
        fs::create_dir_all(target.path())?;
        for file in WALLET_FILES {
            let from = self.root.join(file);
            if from.exists() {
                fs::rename(&from, target.path().join(file))
                    .with_context(|| format!("could not move {}", from.display()))?;
            }
        }
        Ok(Some(format!(
            "moved your existing wallet to {} (named \"{DEFAULT_NAME}\")",
            target.path().display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use wallet::{KeySource, MnemonicLength, generate_mnemonic};

    use super::*;
    use crate::session::NewWallet;

    fn setup() -> (tempfile::TempDir, Wallets) {
        let tmp = tempfile::tempdir().unwrap();
        let wallets = Wallets::new(Some(tmp.path().to_path_buf()), Network::Regtest);
        (tmp, wallets)
    }

    fn create(wallets: &Wallets, name: &str) {
        let phrase = generate_mnemonic(MnemonicLength::Words12).unwrap();
        session::create(
            &wallets.dir(name),
            Network::Regtest,
            NewWallet {
                keys: KeySource::mnemonic(phrase.as_str()),
                birthday: None,
                password: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn names_are_validated() {
        for ok in ["alice", "bob-2", "my_wallet", "a"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "Alice", "a b", "../x", "-x", &"x".repeat(33)] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn create_list_and_choose_the_default() {
        let (_tmp, wallets) = setup();
        assert_eq!(wallets.default_name(), "default", "nothing yet");
        create(&wallets, "bob");
        assert_eq!(wallets.default_name(), "bob", "the only wallet");
        create(&wallets, "alice");
        assert_eq!(wallets.names(), ["alice", "bob"]);
        assert_eq!(
            wallets.default_name(),
            "default",
            "two wallets, none chosen"
        );
        wallets.set_default("alice").unwrap();
        assert_eq!(wallets.default_name(), "alice");
        assert_eq!(wallets.resolve(Some("bob")).unwrap(), "bob");
        assert!(wallets.resolve(Some("Bad Name")).is_err());
        assert!(wallets.set_default("nobody").is_err());

        wallets.dir("bob").record_sync(250_000, 102).unwrap();
        let list = wallets.list();
        assert_eq!(list.len(), 2);
        assert!(list[0].is_default && list[0].name == "alice");
        assert_eq!(list[1].last_balance_sat, Some(250_000));
    }

    #[test]
    fn rename_and_remove_keep_the_default_consistent() {
        let (_tmp, wallets) = setup();
        create(&wallets, "alice");
        create(&wallets, "bob");
        wallets.set_default("alice").unwrap();
        wallets.rename("alice", "carol").unwrap();
        assert_eq!(wallets.default_name(), "carol");
        assert!(wallets.rename("carol", "bob").is_err(), "taken");
        wallets.remove("carol").unwrap();
        assert_eq!(wallets.names(), ["bob"]);
        assert_eq!(wallets.default_name(), "bob");
        assert!(wallets.remove("carol").is_err());
    }

    #[test]
    fn an_old_single_wallet_is_moved_to_default() {
        let (_tmp, wallets) = setup();
        let legacy = DataDir::at(wallets.root().to_path_buf());
        let phrase = generate_mnemonic(MnemonicLength::Words12).unwrap();
        session::create(
            &legacy,
            Network::Regtest,
            NewWallet {
                keys: KeySource::mnemonic(phrase.as_str()),
                birthday: Some(5),
                password: None,
            },
        )
        .unwrap();

        let message = wallets.migrate_legacy().unwrap().unwrap();
        assert!(message.contains("default"));
        assert!(wallets.exists("default"));
        assert!(!wallets.root().join("wallet.sqlite").exists());
        assert_eq!(
            wallets.dir("default").load_meta().unwrap().birthday,
            Some(5)
        );
        assert!(wallets.migrate_legacy().unwrap().is_none(), "only once");
        // Still opens with its keys.
        let keys = match session::stored_keys(&wallets.dir("default")).unwrap() {
            session::StoredKeys::Plain(k) => k,
            _ => panic!("expected plain keys"),
        };
        assert!(
            session::open(&wallets.dir("default"), Network::Regtest, Some(keys))
                .unwrap()
                .can_sign()
        );
    }
}
