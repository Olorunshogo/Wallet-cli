//! Where chain data comes from, shared by the CLI commands and the TUI.
//!
//! - `external`: a Bitcoin Core you run, reached with the `--rpc-*` settings.
//! - `local`: a regtest `bitcoind` this app runs in the background, **shared
//!   by every regtest wallet** so they are on the same chain. Its data lives
//!   in the node directory (default `.wallet/regtest/node`).
//! - `none`: no node; only commands that work offline are available.
//!
//! The local node is started on demand. Every program using it holds a
//! [`Lease`]; when the last one exits (a CLI command finishing, or `q` in the
//! TUI) a node that was started on demand is stopped. A node started with
//! `node start` is *pinned* and keeps running until `node stop`.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use wallet::bitcoin::{Block, BlockHash, Network, Transaction};
use wallet::rpc::{RpcAuth, RpcClient};
use wallet::{BlockId, BlockSource, SourceError};

use crate::config::RpcArgs;

/// Where chain data comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum NodeMode {
    /// A Bitcoin Core node you run, reached with the `--rpc-*` settings.
    #[default]
    External,
    /// A regtest `bitcoind` this app runs in the background, shared by every
    /// regtest wallet.
    Local,
    /// No node: work offline with what is stored locally.
    None,
}

/// Default RPC port of the local node; the P2P port is the next one. Chosen
/// to avoid a `bitcoind` you may run yourself on 18443.
pub const DEFAULT_LOCAL_PORT: u16 = 18543;

/// How long to wait for the local node to start or stop.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

// === Connection used by commands

/// A connected node. Holding it keeps a local node alive; dropping it lets
/// an on-demand node stop once nobody else uses it.
pub struct Node {
    client: RpcClient,
    label: String,
    keepalive: Option<Box<dyn std::any::Any + Send>>,
}

impl Node {
    /// The RPC client (a `BlockSource`, `Broadcaster` and `FeeEstimator`).
    pub fn rpc(&self) -> &RpcClient {
        &self.client
    }

    /// Human description, e.g. "Bitcoin Core at http://127.0.0.1:18443".
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Split into the client, its label and what keeps the node alive (keep
    /// that as long as the client is used).
    pub fn into_parts(self) -> (RpcClient, String, Option<Box<dyn std::any::Any + Send>>) {
        (self.client, self.label, self.keepalive)
    }
}

/// Where the local node lives and listens.
#[derive(Debug, Clone)]
pub struct LocalNode {
    /// Its data directory.
    pub dir: PathBuf,
    /// Its RPC port.
    pub port: u16,
}

/// Connect according to `mode`.
pub fn connect(mode: NodeMode, rpc: &RpcArgs, local: &LocalNode, network: Network) -> Result<Node> {
    match mode {
        NodeMode::External => Ok(Node {
            client: rpc.connect(network)?,
            label: format!("Bitcoin Core at {}", rpc.url(network)),
            keepalive: None,
        }),
        NodeMode::Local => {
            if network != Network::Regtest {
                bail!("a local node only runs regtest; drop --network or use --node external");
            }
            let shared = Shared::new(local.dir.clone());
            // Lease first, so a program leaving right now does not stop the
            // node between our start and our lease.
            let lease = shared.lease()?;
            shared.start(local.port, false)?;
            let client = shared.client()?;
            Ok(Node {
                label: format!(
                    "local regtest node (port {})",
                    shared.port().unwrap_or(local.port)
                ),
                client,
                keepalive: Some(Box::new(lease)),
            })
        }
        NodeMode::None => bail!(
            "this command needs a node, but --node none is set; \
             use --node local (regtest) or --node external"
        ),
    }
}

// === The shared local node

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeFile {
    rpc_port: u16,
    pinned: bool,
}

/// What `node status` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Not running.
    Stopped,
    /// Running and answering.
    Running {
        /// RPC port.
        port: u16,
        /// Chain height.
        height: u32,
        /// Started with `node start` (stays up until `node stop`).
        pinned: bool,
        /// Programs using it right now.
        users: usize,
    },
}

/// The local regtest node in `dir`, which may or may not be running.
#[derive(Debug, Clone)]
pub struct Shared {
    dir: PathBuf,
}

impl Shared {
    /// The node whose data lives in `dir`.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Its data directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn info_path(&self) -> PathBuf {
        self.dir.join("node.json")
    }

    fn leases_dir(&self) -> PathBuf {
        self.dir.join("leases")
    }

    fn cookie(&self) -> PathBuf {
        self.dir.join("regtest").join(".cookie")
    }

    fn info(&self) -> Option<NodeFile> {
        let text = std::fs::read_to_string(self.info_path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn write_info(&self, info: &NodeFile) -> Result<()> {
        std::fs::write(self.info_path(), serde_json::to_string_pretty(info)?)
            .context("could not write node.json")
    }

    /// Its RPC port, if it has been started before.
    pub fn port(&self) -> Option<u16> {
        self.info().map(|i| i.rpc_port)
    }

    fn connect_to(&self, port: u16, timeout: Duration) -> Result<RpcClient> {
        let url = format!("http://127.0.0.1:{port}");
        Ok(RpcClient::with_timeout(
            &url,
            RpcAuth::Cookie(self.cookie()),
            timeout,
        )?)
    }

    /// A client, if the node answers right now.
    fn alive(&self) -> Option<(RpcClient, u32)> {
        let port = self.port()?;
        let client = self.connect_to(port, Duration::from_secs(3)).ok()?;
        let height = client.tip().ok()?.height;
        Some((client, height))
    }

    /// A client for the running node.
    pub fn client(&self) -> Result<RpcClient> {
        let port = self.port().context("the local node has not been started")?;
        self.connect_to(port, wallet::rpc::DEFAULT_TIMEOUT)
    }

    /// Running or not, and who uses it.
    pub fn status(&self) -> Status {
        match (self.alive(), self.info()) {
            (Some((_, height)), Some(info)) => Status::Running {
                port: info.rpc_port,
                height,
                pinned: info.pinned,
                users: self.live_leases(),
            },
            _ => Status::Stopped,
        }
    }

    /// Start the node if it is not running. `pinned` keeps it running after
    /// its users exit (for `node start`); pinning a running node also pins it.
    /// Returns true if this call started it.
    pub fn start(&self, port: u16, pinned: bool) -> Result<bool> {
        if self.alive().is_some() {
            if pinned && let Some(mut info) = self.info() {
                info.pinned = true;
                self.write_info(&info)?;
            }
            return Ok(false);
        }
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("could not create {}", self.dir.display()))?;
        let exe = bitcoind()?;
        self.write_info(&NodeFile {
            rpc_port: port,
            pinned,
        })?;
        let output = Command::new(&exe)
            .arg("-regtest")
            .arg("-daemon")
            .arg(format!("-datadir={}", self.dir.display()))
            .arg(format!("-rpcport={port}"))
            .arg(format!("-port={}", port + 1))
            .arg("-listen=0")
            .arg("-fallbackfee=0.0001")
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("could not run {}", exe.display()))?;
        // Another program may have started it a moment ago; that is fine as
        // long as it ends up answering.
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        while Instant::now() < deadline {
            if self.alive().is_some() {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "the local node did not start on port {port}: {}{}",
            stderr.trim(),
            self.log_tail()
                .map(|t| format!("\nlast lines of its log:\n{t}"))
                .unwrap_or_default()
        )
    }

    /// Stop the node (pinned or not). Returns false if it was not running.
    pub fn stop(&self) -> Result<bool> {
        let Some((client, _)) = self.alive() else {
            return Ok(false);
        };
        client.stop().context("the local node refused to stop")?;
        let pid_file = self.dir.join("regtest").join("bitcoind.pid");
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        while Instant::now() < deadline {
            if !pid_file.exists() && self.alive().is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        if let Some(mut info) = self.info() {
            info.pinned = false;
            self.write_info(&info)?;
        }
        let _ = std::fs::remove_dir_all(self.leases_dir());
        Ok(true)
    }

    /// Stop the node and delete its chain. Wallets are not touched.
    pub fn reset(&self) -> Result<()> {
        self.stop()?;
        if self.dir.exists() {
            std::fs::remove_dir_all(&self.dir)
                .with_context(|| format!("could not delete {}", self.dir.display()))?;
        }
        Ok(())
    }

    /// Register this process as a user of the node.
    pub fn lease(&self) -> Result<Lease> {
        std::fs::create_dir_all(self.leases_dir())?;
        let path = self.leases_dir().join(std::process::id().to_string());
        std::fs::write(&path, b"")?;
        Ok(Lease {
            shared: self.clone(),
            path,
        })
    }

    /// Processes still using the node; leases of processes that died are
    /// cleaned up on the way.
    fn live_leases(&self) -> usize {
        let Ok(entries) = std::fs::read_dir(self.leases_dir()) else {
            return 0;
        };
        entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let alive = e
                    .file_name()
                    .to_str()
                    .and_then(|n| n.parse::<u32>().ok())
                    .is_some_and(pid_alive);
                if !alive {
                    let _ = std::fs::remove_file(e.path());
                }
                alive
            })
            .count()
    }

    fn log_tail(&self) -> Option<String> {
        let log = std::fs::read_to_string(self.dir.join("regtest").join("debug.log")).ok()?;
        let lines: Vec<&str> = log.lines().collect();
        Some(lines[lines.len().saturating_sub(5)..].join("\n"))
    }
}

/// Keeps the local node up while held. When the last lease of an on-demand
/// (unpinned) node is dropped, the node is stopped.
pub struct Lease {
    shared: Shared,
    path: PathBuf,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let pinned = self.shared.info().is_some_and(|i| i.pinned);
        if !pinned && self.shared.live_leases() == 0 {
            let _ = self.shared.stop();
        }
    }
}

fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The `bitcoind` to run: `BITCOIND_EXE`, then the build-time download,
/// then `PATH`.
#[cfg(feature = "local-node")]
fn bitcoind() -> Result<PathBuf> {
    corepc_node::exe_path()
        .map(PathBuf::from)
        .map_err(|e| anyhow::anyhow!(e))
        .context("no bitcoind found; set BITCOIND_EXE or install Bitcoin Core")
}

#[cfg(not(feature = "local-node"))]
fn bitcoind() -> Result<PathBuf> {
    which_bitcoind().context("no bitcoind found; set BITCOIND_EXE or install Bitcoin Core")
}

#[cfg(not(feature = "local-node"))]
fn which_bitcoind() -> Option<PathBuf> {
    if let Some(exe) = std::env::var_os("BITCOIND_EXE") {
        return Some(PathBuf::from(exe));
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join("bitcoind"))
            .find(|p| p.exists())
    })
}

/// A throwaway regtest `bitcoind` (for `tui --demo`), stopped when dropped.
#[cfg(feature = "local-node")]
pub fn start_throwaway() -> Result<(corepc_node::Node, RpcClient), wallet::BoxError> {
    let mut conf = corepc_node::Conf::default();
    conf.wallet = None;
    let node = corepc_node::Node::with_conf(corepc_node::exe_path()?, &conf)?;
    let client = RpcClient::new(
        &node.rpc_url(),
        RpcAuth::Cookie(node.params.cookie_file.clone()),
    )?;
    Ok((node, client))
}

// === Sync progress

/// Wraps any block source to report how many blocks a sync has fetched.
///
/// The same decorator pattern as the library's `custom_backend` example: the
/// wallet sees a normal [`BlockSource`].
pub struct Progress<'a, S: BlockSource + ?Sized, F: Fn(u32)> {
    inner: &'a S,
    on_block: F,
    fetched: Cell<u32>,
    last_report: Cell<Option<Instant>>,
    every: Duration,
}

impl<'a, S: BlockSource + ?Sized, F: Fn(u32)> Progress<'a, S, F> {
    /// Report at most once per `every` (plus the first block).
    pub fn new(inner: &'a S, every: Duration, on_block: F) -> Self {
        Self {
            inner,
            on_block,
            fetched: Cell::new(0),
            last_report: Cell::new(None),
            every,
        }
    }

    /// Blocks fetched so far.
    #[cfg(test)]
    pub fn fetched(&self) -> u32 {
        self.fetched.get()
    }
}

impl<S: BlockSource + ?Sized, F: Fn(u32)> BlockSource for Progress<'_, S, F> {
    fn tip(&self) -> Result<BlockId, SourceError> {
        self.inner.tip()
    }

    fn block_hash(&self, height: u32) -> Result<BlockHash, SourceError> {
        self.inner.block_hash(height)
    }

    fn block(&self, hash: &BlockHash) -> Result<Block, SourceError> {
        let block = self.inner.block(hash)?;
        let fetched = self.fetched.get() + 1;
        self.fetched.set(fetched);
        let now = Instant::now();
        let due = self
            .last_report
            .get()
            .is_none_or(|last| now.duration_since(last) >= self.every);
        if due {
            self.last_report.set(Some(now));
            (self.on_block)(fetched);
        }
        Ok(block)
    }

    fn mempool(&self) -> Result<Vec<(Transaction, u64)>, SourceError> {
        self.inner.mempool()
    }
}
