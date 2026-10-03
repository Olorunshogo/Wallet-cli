use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde::{Deserialize, Serialize};
use wallet::KeySource;
use wallet::bitcoin::Network;
use wallet::rpc::{RpcAuth, RpcClient};
use zeroize::Zeroizing;

// === Data directory

/// Layout of the CLI's data directory:
///
/// - `wallet.sqlite`: wallet state (public data only)
/// - `mnemonic`, `passphrase`: secrets, owner-readable only
/// - `meta.json`: CLI settings such as the birthday height
#[derive(Debug, Clone)]
pub struct DataDir {
    path: PathBuf,
}

/// Small per-wallet facts kept next to the database.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Meta {
    /// First block to scan on the initial sync.
    pub birthday: Option<u32>,
    /// Total balance in sats at the last sync, for wallet lists.
    pub last_balance_sat: Option<u64>,
    /// Chain height at the last sync.
    pub last_height: Option<u32>,
}

impl DataDir {
    /// A wallet directory at exactly `path`.
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn db(&self) -> PathBuf {
        self.path.join("wallet.sqlite")
    }

    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.path)
            .with_context(|| format!("could not create {}", self.path.display()))
    }

    pub fn save_meta(&self, birthday: Option<u32>) -> Result<()> {
        self.update_meta(|m| m.birthday = birthday)
    }

    /// Remember the balance and height after a sync, for wallet lists.
    pub fn record_sync(&self, balance_sat: u64, height: u32) -> Result<()> {
        self.update_meta(|m| {
            m.last_balance_sat = Some(balance_sat);
            m.last_height = Some(height);
        })
    }

    fn update_meta(&self, change: impl FnOnce(&mut Meta)) -> Result<()> {
        self.ensure()?;
        let mut meta = self.load_meta()?;
        change(&mut meta);
        let json = serde_json::to_string_pretty(&meta)?;
        fs::write(self.path.join("meta.json"), json).context("could not write meta.json")
    }

    pub fn load_meta(&self) -> Result<Meta> {
        match fs::read_to_string(self.path.join("meta.json")) {
            Ok(s) => Ok(serde_json::from_str(&s).context("meta.json is invalid")?),
            Err(_) => Ok(Meta::default()),
        }
    }

    pub fn save_mnemonic(&self, phrase: &str) -> Result<()> {
        self.write_secret("mnemonic", phrase)
    }

    pub fn save_passphrase(&self, passphrase: &str) -> Result<()> {
        self.write_secret("passphrase", passphrase)
    }

    /// Save the mnemonic (and passphrase, if any) encrypted with AES-256-GCM.
    ///
    /// The key is derived from `password` with Argon2id. File format:
    /// `[salt (16 bytes)][nonce (12 bytes)][ciphertext]`, where the plaintext
    /// is the mnemonic, followed by `\n` and the passphrase when one is set.
    pub fn save_mnemonic_encrypted(
        &self,
        phrase: &str,
        passphrase: Option<&str>,
        password: &str,
    ) -> Result<()> {
        use aes_gcm::aead::{Aead, KeyInit};
        use aes_gcm::{Aes256Gcm, Nonce};
        use argon2::Argon2;
        use rand::RngCore;

        self.ensure()?;
        let mut salt = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut salt);

        let mut key_bytes = Zeroizing::new([0u8; 32]);
        Argon2::default()
            .hash_password_into(password.as_bytes(), &salt, key_bytes.as_mut())
            .map_err(|e| anyhow::anyhow!("argon2 error: {}", e))?;

        let cipher = Aes256Gcm::new_from_slice(key_bytes.as_ref())
            .map_err(|e| anyhow::anyhow!("cipher init error: {}", e))?;
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let mut plaintext = Zeroizing::new(phrase.to_owned());
        if let Some(p) = passphrase {
            plaintext.push('\n');
            plaintext.push_str(p);
        }
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| anyhow::anyhow!("encryption error: {}", e))?;

        let mut file_bytes = Vec::with_capacity(salt.len() + nonce_bytes.len() + ciphertext.len());
        file_bytes.extend_from_slice(&salt);
        file_bytes.extend_from_slice(&nonce_bytes);
        file_bytes.extend_from_slice(&ciphertext);
        return write_private(&self.encrypted_path(), &file_bytes);
    }

    /// Decrypt the stored keys.
    pub fn load_mnemonic_encrypted(&self, password: &str) -> Result<KeySource> {
        use aes_gcm::aead::{Aead, KeyInit};
        use aes_gcm::{Aes256Gcm, Nonce};
        use argon2::Argon2;

        let path = self.encrypted_path();
        let file_bytes =
            fs::read(&path).with_context(|| format!("could not read {}", path.display()))?;
        if file_bytes.len() < 28 {
            bail!("encrypted mnemonic file is too short");
        }
        let (salt, rest) = file_bytes.split_at(16);
        let (nonce_bytes, ciphertext) = rest.split_at(12);

        let mut key_bytes = Zeroizing::new([0u8; 32]);
        Argon2::default()
            .hash_password_into(password.as_bytes(), salt, key_bytes.as_mut())
            .map_err(|e| anyhow::anyhow!("argon2 error: {}", e))?;
        let cipher = Aes256Gcm::new_from_slice(key_bytes.as_ref())
            .map_err(|e| anyhow::anyhow!("cipher init error: {}", e))?;
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
                .map_err(|_| {
                    anyhow::anyhow!("decryption failed: wrong password or corrupt file")
                })?,
        );
        let text = std::str::from_utf8(&plaintext).context("decrypted data is not valid UTF-8")?;
        return Ok(match text.split_once('\n') {
            Some((phrase, passphrase)) => KeySource::mnemonic_with_passphrase(phrase, passphrase),
            None => KeySource::mnemonic(text),
        });
    }

    /// Whether the keys are stored encrypted (a password is needed to sign).
    pub fn is_encrypted(&self) -> bool {
        self.encrypted_path().exists()
    }

    fn encrypted_path(&self) -> PathBuf {
        self.path.join("mnemonic.enc")
    }

    /// Keys for signing, or `None` to open the wallet watch-only.
    pub fn load_keys(&self) -> Result<Option<KeySource>> {
        let Ok(phrase) = fs::read_to_string(self.path.join("mnemonic")) else {
            return Ok(None);
        };
        let phrase = phrase.trim();
        Ok(Some(
            match fs::read_to_string(self.path.join("passphrase")) {
                Ok(p) => KeySource::mnemonic_with_passphrase(phrase, p.trim_end_matches('\n')),
                Err(_) => KeySource::mnemonic(phrase),
            },
        ))
    }

    fn write_secret(&self, name: &str, value: &str) -> Result<()> {
        self.ensure()?;
        write_private(&self.path.join(name), value.as_bytes())
    }
}

/// Write a file readable only by its owner. On unix the file is created with
/// mode 0600, so the secret is never briefly readable by other users.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;

    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("could not write {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("could not write {}", path.display()))?;
    // Tightens permissions if the file already existed with looser ones.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

// === Node connection

/// RPC URL of the first bitcoind node in a Polar network.
pub const POLAR_URL: &str = "http://127.0.0.1:18443";

#[derive(Args, Clone, Debug, Default)]
pub struct RpcArgs {
    /// Bitcoin Core RPC URL. Defaults to localhost on the network's port.
    #[arg(long, global = true, env = "WALLET_RPC_URL")]
    rpc_url: Option<String>,
    /// Path to the node's .cookie file.
    #[arg(
        long,
        global = true,
        env = "WALLET_RPC_COOKIE",
        conflicts_with = "rpc_user"
    )]
    rpc_cookie: Option<PathBuf>,
    #[arg(long, global = true, env = "WALLET_RPC_USER", requires = "rpc_pass")]
    rpc_user: Option<String>,
    #[arg(long, global = true, env = "WALLET_RPC_PASS", hide_env_values = true)]
    rpc_pass: Option<String>,
}

impl RpcArgs {
    /// The node URL: `--rpc-url`, or localhost on the network's default port.
    pub fn url(&self, network: Network) -> String {
        self.rpc_url
            .clone()
            .unwrap_or_else(|| format!("http://127.0.0.1:{}", default_port(network)))
    }

    /// These settings with Polar's defaults filled in: its first bitcoind
    /// node's RPC port and login. Anything set explicitly wins.
    pub fn polar(&self) -> RpcArgs {
        RpcArgs {
            rpc_url: self.rpc_url.clone().or_else(|| Some(POLAR_URL.to_string())),
            rpc_cookie: None,
            rpc_user: self.rpc_user.clone().or_else(|| Some("polaruser".into())),
            rpc_pass: self.rpc_pass.clone().or_else(|| Some("polarpass".into())),
        }
    }

    /// Whether any `--rpc-*` setting was given, i.e. the user points at a
    /// node of their own (Bitcoin Core, Polar, ...).
    pub fn is_set(&self) -> bool {
        self.rpc_url.is_some()
            || self.rpc_cookie.is_some()
            || self.rpc_user.is_some()
            || self.rpc_pass.is_some()
    }

    pub fn connect(&self, network: Network) -> Result<RpcClient> {
        let url = self.url(network);
        let auth = match (&self.rpc_user, &self.rpc_pass, &self.rpc_cookie) {
            (Some(user), Some(pass), _) => RpcAuth::UserPass {
                user: user.clone(),
                pass: pass.clone(),
            },
            (_, _, Some(cookie)) => RpcAuth::Cookie(cookie.clone()),
            _ => RpcAuth::Cookie(default_cookie(network)?),
        };
        RpcClient::new(&url, auth).with_context(|| format!("could not connect to {url}"))
    }
}

fn default_port(network: Network) -> u16 {
    match network {
        Network::Bitcoin => 8332,
        Network::Testnet => 18332,
        Network::Testnet4 => 48332,
        Network::Signet => 38332,
        _ => 18443,
    }
}

fn default_cookie(network: Network) -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set; pass --rpc-cookie")?;
    let base = PathBuf::from(home).join(".bitcoin");
    Ok(match network {
        Network::Bitcoin => base.join(".cookie"),
        Network::Testnet => base.join("testnet3").join(".cookie"),
        other => base.join(other.to_core_arg()).join(".cookie"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polar() -> RpcArgs {
        // Nothing listens on port 1, so connecting fails fast.
        RpcArgs {
            rpc_url: Some("http://127.0.0.1:1".into()),
            rpc_user: Some("polaruser".into()),
            rpc_pass: Some("polarpass".into()),
            ..RpcArgs::default()
        }
    }

    #[test]
    fn regtest_uses_the_local_node_unless_a_node_is_configured() {
        use crate::node::{NodeMode, default_mode};
        assert_eq!(
            default_mode(Network::Regtest, &RpcArgs::default()),
            NodeMode::Local
        );
        assert_eq!(default_mode(Network::Regtest, &polar()), NodeMode::External);
        assert_eq!(
            default_mode(Network::Signet, &RpcArgs::default()),
            NodeMode::External
        );
    }

    #[test]
    fn an_unreachable_node_says_how_to_fix_it() {
        let mut connect = crate::tui::chain::rpc(polar(), Network::Regtest);
        let err = connect().err().expect("nothing listens there").to_string();
        assert!(
            err.contains("no Bitcoin Core at http://127.0.0.1:1"),
            "{err}"
        );
        assert!(err.contains("--node local"), "{err}");
        assert!(err.contains("WALLET_RPC_USER"), "{err}");
    }

    #[test]
    fn polar_defaults_fill_in_url_and_login_unless_set() {
        let defaults = RpcArgs::default().polar();
        assert_eq!(defaults.url(Network::Regtest), super::POLAR_URL);
        assert_eq!(defaults.rpc_user.as_deref(), Some("polaruser"));
        assert_eq!(defaults.rpc_pass.as_deref(), Some("polarpass"));

        // An explicit --rpc-* setting always wins over Polar's defaults.
        let custom = RpcArgs {
            rpc_url: Some("http://127.0.0.1:19443".into()),
            ..RpcArgs::default()
        }
        .polar();
        assert_eq!(custom.url(Network::Regtest), "http://127.0.0.1:19443");
        assert_eq!(
            custom.rpc_user.as_deref(),
            Some("polaruser"),
            "still filled in"
        );
    }

    #[test]
    fn an_unreachable_polar_node_says_to_start_polar_or_use_local() {
        let mut connect = crate::tui::chain::polar(RpcArgs {
            rpc_url: Some("http://127.0.0.1:1".into()),
            ..RpcArgs::default()
        });
        let err = connect().err().expect("nothing listens there").to_string();
        assert!(err.contains("could not connect to the Polar node"), "{err}");
        assert!(err.contains("Is your Polar network started"), "{err}");
        assert!(err.contains("WALLET_NODE=local"), "{err}");
    }

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

    #[test]
    fn encrypted_mnemonic_round_trips() {
        let (_tmp, dir) = datadir();
        let phrase = fresh_phrase();
        dir.save_mnemonic_encrypted(&phrase, None, "correct horse")
            .unwrap();
        match dir.load_mnemonic_encrypted("correct horse").unwrap() {
            KeySource::Mnemonic {
                phrase: loaded,
                passphrase,
            } => {
                assert_eq!(loaded.as_str(), phrase);
                assert!(passphrase.is_none());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn encrypted_passphrase_round_trips() {
        let (_tmp, dir) = datadir();
        let original = fresh_phrase();
        dir.save_mnemonic_encrypted(&original, Some("my passphrase"), "pw")
            .unwrap();
        match dir.load_mnemonic_encrypted("pw").unwrap() {
            KeySource::Mnemonic { phrase, passphrase } => {
                assert_eq!(phrase.as_str(), original);
                assert_eq!(passphrase.unwrap().as_str(), "my passphrase");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn encrypted_file_does_not_contain_the_mnemonic() {
        let (_tmp, dir) = datadir();
        let phrase = fresh_phrase();
        dir.save_mnemonic_encrypted(&phrase, None, "pw").unwrap();
        let raw = fs::read(dir.path().join("mnemonic.enc")).unwrap();
        let first_word = phrase.split(' ').next().unwrap().as_bytes();
        assert!(!raw.windows(phrase.len()).any(|w| w == phrase.as_bytes()));
        assert!(!raw.windows(first_word.len()).any(|w| w == first_word));
    }

    #[test]
    fn wrong_password_is_rejected() {
        let (_tmp, dir) = datadir();
        dir.save_mnemonic_encrypted(&fresh_phrase(), None, "right")
            .unwrap();
        let err = dir.load_mnemonic_encrypted("wrong").unwrap_err();
        assert!(err.to_string().contains("wrong password"), "{err}");
    }

    #[test]
    fn tampered_file_is_rejected() {
        let (_tmp, dir) = datadir();
        dir.save_mnemonic_encrypted(&fresh_phrase(), None, "pw")
            .unwrap();
        let path = dir.path().join("mnemonic.enc");
        let mut raw = fs::read(&path).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0x01;
        fs::write(&path, &raw).unwrap();
        assert!(dir.load_mnemonic_encrypted("pw").is_err());

        fs::write(&path, [0u8; 10]).unwrap();
        let err = dir.load_mnemonic_encrypted("pw").unwrap_err();
        assert!(err.to_string().contains("too short"), "{err}");
    }

    #[test]
    fn same_input_encrypts_differently_each_time() {
        let (_tmp, dir) = datadir();
        let phrase = fresh_phrase();
        dir.save_mnemonic_encrypted(&phrase, None, "pw").unwrap();
        let first = fs::read(dir.path().join("mnemonic.enc")).unwrap();
        dir.save_mnemonic_encrypted(&phrase, None, "pw").unwrap();
        let second = fs::read(dir.path().join("mnemonic.enc")).unwrap();
        assert_ne!(first, second, "fresh salt and nonce every time");
    }

    #[cfg(unix)]
    #[test]
    fn secrets_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, dir) = datadir();
        dir.save_mnemonic(&fresh_phrase()).unwrap();
        dir.save_mnemonic_encrypted(&fresh_phrase(), None, "pw")
            .unwrap();
        for name in ["mnemonic", "mnemonic.enc"] {
            let mode = fs::metadata(dir.path().join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{name}");
        }
    }
}
