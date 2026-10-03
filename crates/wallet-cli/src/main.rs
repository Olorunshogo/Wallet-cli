//! Command-line demo client for the wallet library.
//!
//! Everything wallet-related goes through the `wallet` crate's public API;
//! this binary only handles arguments, files and output formatting.

// Explicit returns are a project convention; clippy's needless_return lint
// disagrees, so allow it crate-wide.
#![allow(clippy::needless_return)]

mod activity;
mod config;
mod node;
mod output;
mod pager;
mod palette;
mod session;
mod style;
mod tui;
mod wallets;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use wallet::bitcoin::address::NetworkUnchecked;
use wallet::bitcoin::{Address, Amount, FeeRate, Network, Txid};
use wallet::rpc::RpcClient;
use wallet::{
    BlockSource, Broadcaster, CoinSelection, FeeEstimateError, FeeEstimator, KeySource,
    MnemonicLength, Recipient, SignOutcome, SyncReport, TxRequest, Wallet, generate_mnemonic,
};

use crate::activity::Activity;
use crate::config::{DataDir, RpcArgs};
use crate::node::{LocalNode, NodeMode};
use crate::output::Output;
use crate::pager::Page;
use crate::tui::Target;
use crate::wallets::Wallets;
use zeroize::Zeroizing;

/// Used when the node has no fee estimate yet (fresh regtest or signet).
const FALLBACK_FEE_RATE: u64 = 1;

#[derive(Parser)]
#[command(
    name = "wallet-cli",
    version,
    about = "Non-custodial BIP84 wallet on top of the wallet library"
)]
struct Cli {
    /// Network to operate on.
    #[arg(long, short, global = true, default_value = "regtest", value_parser = parse_network, env = "WALLET_NETWORK")]
    network: Network,

    /// Wallet to use, by name (see `wallets`). Defaults to the one set with
    /// `wallets use`, or the only one.
    #[arg(long, short = 'w', global = true, env = "WALLET_NAME")]
    wallet: Option<String>,

    /// Use the wallet in this exact directory instead of a named one.
    #[arg(long, global = true, env = "WALLET_DATADIR", conflicts_with = "wallet")]
    datadir: Option<PathBuf>,

    /// Print machine-readable JSON instead of text.
    #[arg(long, global = true)]
    json: bool,

    /// Encrypt the mnemonic with a password when creating or restoring.
    #[arg(long, global = true, env = "WALLET_ENCRYPT")]
    encrypt: bool,

    /// Required to use mainnet, where mistakes cost real money.
    #[arg(long, global = true, env = "WALLET_ALLOW_MAINNET")]
    allow_mainnet: bool,

    /// Where chain data comes from: your Bitcoin Core (`external`), the shared
    /// local regtest node this app runs for all wallets (`local`), Polar's
    /// regtest node (`polar`), or none. Default: `local` on regtest when no
    /// `--rpc-*` setting is given, otherwise `external`.
    #[arg(long, global = true, value_enum, env = "WALLET_NODE")]
    node: Option<NodeMode>,

    /// Data directory of the local node. Default: .wallet/regtest/node.
    #[arg(long, global = true, env = "WALLET_NODE_DIR")]
    node_dir: Option<PathBuf>,

    /// RPC port of the local node (P2P uses the next port).
    #[arg(long, global = true, env = "WALLET_NODE_PORT", default_value_t = node::DEFAULT_LOCAL_PORT)]
    node_port: u16,

    #[command(flatten)]
    rpc: RpcArgs,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new wallet with a freshly generated mnemonic.
    Init {
        /// Number of words: 12, 15, 18, 21 or 24.
        #[arg(long, env = "WALLET_WORDS", default_value_t = 12, value_parser = parse_word_count)]
        words: usize,
    },
    /// Restore a wallet from an existing mnemonic.
    Restore {
        /// The BIP39 mnemonic, in quotes.
        #[arg(long, env = "WALLET_MNEMONIC")]
        mnemonic: String,
        /// Optional BIP39 passphrase.
        #[arg(long, env = "WALLET_PASSPHRASE")]
        passphrase: Option<String>,
        /// Block height to start scanning from on the first sync.
        #[arg(long)]
        birthday: Option<u32>,
    },
    /// Show a receive address.
    Address {
        /// Reveal a fresh address even if the current one is unused.
        #[arg(long)]
        new: bool,
    },
    /// Update the wallet from the node.
    Sync,
    /// Show confirmed, unconfirmed and immature balance.
    Balance,
    /// List unspent outputs, a page at a time.
    Utxos(PageArgs),
    /// List wallet transactions, newest first, a page at a time.
    History(PageArgs),
    /// Pay an address.
    Send {
        address: Address<NetworkUnchecked>,
        /// Amount: sats (`50000`) or BTC with a suffix (`0.5btc`).
        #[arg(value_parser = tui::validate::parse_amount)]
        amount: Amount,
        /// Fee rate in sat/vB. Estimated from the node when omitted.
        #[arg(long)]
        fee_rate: Option<u64>,
        /// Confirmation target in blocks, used for fee estimation.
        #[arg(long, default_value_t = 6)]
        target: u16,
        #[arg(long, value_enum)]
        selection: Option<Selection>,
        /// Build and sign but print the transaction instead of broadcasting.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show the confirmation status of a transaction.
    Status { txid: Txid },
    /// Print the public descriptors (for a watch-only copy).
    Descriptors,
    /// Export an unsigned PSBT for external signing.
    ExportPsbt {
        address: Address<NetworkUnchecked>,
        /// Amount: sats (`50000`) or BTC with a suffix (`0.5btc`).
        #[arg(value_parser = tui::validate::parse_amount)]
        amount: Amount,
        /// Fee rate in sat/vB.
        #[arg(long)]
        fee_rate: Option<u64>,
        /// Output file path for the PSBT.
        #[arg(long, default_value = "psbt.dat")]
        output: PathBuf,
    },
    /// Sign a PSBT file and optionally broadcast it.
    SignPsbt {
        /// Input PSBT file path.
        #[arg(long, default_value = "psbt.dat")]
        input: PathBuf,
        /// Broadcast after signing.
        #[arg(long)]
        broadcast: bool,
    },
    /// Mine regtest blocks to this wallet (or `--to`), then sync. Works with
    /// `--node local` or your own regtest node.
    Mine {
        /// How many blocks. 101 makes the first reward spendable.
        #[arg(default_value_t = 1)]
        blocks: u32,
        /// Send the rewards here instead of to this wallet.
        #[arg(long)]
        to: Option<Address<NetworkUnchecked>>,
    },
    /// Open the interactive terminal interface.
    Tui(tui::TuiArgs),
    /// List and manage named wallets (default: list).
    Wallets {
        #[command(subcommand)]
        action: Option<WalletsAction>,
    },
    /// Control the shared local regtest node.
    Node {
        #[command(subcommand)]
        action: NodeAction,
    },
    /// Bump the fee of an unconfirmed transaction.
    BumpFee {
        txid: Txid,
        /// New fee rate in sat/vB.
        #[arg(long)]
        fee_rate: u64,
    },
}

#[derive(Subcommand)]
enum WalletsAction {
    /// List wallets with their last-known balance.
    List,
    /// Create a wallet with fresh recovery words.
    Create {
        /// Its name: a-z, 0-9, - and _.
        name: String,
        /// Number of words: 12, 15, 18, 21 or 24.
        #[arg(long, env = "WALLET_WORDS", default_value_t = 12, value_parser = parse_word_count)]
        words: usize,
    },
    /// Make a wallet the default.
    Use { name: String },
    /// Rename a wallet.
    Rename { old: String, new: String },
    /// Delete a wallet's files (its recovery words are the only way back).
    Remove {
        name: String,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        /// Remove even if it still holds coins.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum NodeAction {
    /// Start the node in the background and keep it running until `stop`.
    Start,
    /// Stop the node.
    Stop,
    /// Is it running, at what height, used by how many programs.
    Status,
    /// Stop the node and delete its chain (wallets are kept).
    Reset {
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
}

/// Which part of a long list to print.
#[derive(clap::Args, Debug, Clone, Copy)]
struct PageArgs {
    /// Page to show, starting at 1.
    #[arg(long, default_value_t = 1)]
    page: usize,
    /// Items per page.
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=1000))]
    per_page: u16,
    /// Show everything on one page.
    #[arg(long, conflicts_with_all = ["page", "per_page"])]
    all: bool,
}

impl PageArgs {
    fn apply<'a, T>(&self, items: &'a [T]) -> Page<'a, T> {
        if self.all {
            pager::paginate(items, 1, items.len().max(1))
        } else {
            pager::paginate(items, self.page, usize::from(self.per_page))
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Selection {
    Bnb,
    Largest,
    Oldest,
}

impl From<Selection> for CoinSelection {
    fn from(s: Selection) -> Self {
        match s {
            Selection::Bnb => CoinSelection::BranchAndBound,
            Selection::Largest => CoinSelection::LargestFirst,
            Selection::Oldest => CoinSelection::OldestFirst,
        }
    }
}

pub(crate) fn parse_word_count(s: &str) -> Result<usize, String> {
    let words: usize = s.parse().map_err(|_| format!("{s} is not a number"))?;
    MnemonicLength::from_words(words)
        .map(|_| words)
        .ok_or_else(|| "mnemonics have 12, 15, 18, 21 or 24 words".into())
}

/// Accepts the common names (`bitcoin`, `mainnet`, `testnet`) as well as
/// Bitcoin Core's own (`main`, `test`, `testnet4`, `signet`, `regtest`).
fn parse_network(s: &str) -> Result<Network, String> {
    match s.to_ascii_lowercase().as_str() {
        "bitcoin" | "mainnet" | "main" => Ok(Network::Bitcoin),
        "testnet" | "testnet3" | "test" => Ok(Network::Testnet),
        other => Network::from_core_arg(other).map_err(|_| {
            format!("unknown network {s:?}; use regtest, signet, testnet, testnet4 or bitcoin")
        }),
    }
}

fn main() {
    // Optional `.env` in the working directory (see `.env.example`). Values
    // already set in the environment win, and flags win over both.
    let _ = dotenvy::dotenv();
    let cli = Cli::parse();
    // The TUI logs to a file instead; anything on the terminal would
    // corrupt its screen.
    if !matches!(cli.command, Command::Tui(_)) {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            // stderr keeps `--json` output on stdout clean.
            .with_writer(std::io::stderr)
            .init();
    }

    style::init(cli.json);
    let out = Output::new(cli.json);
    if let Err(err) = run(cli, &out) {
        out.error(&err);
        std::process::exit(1);
    }
}

fn run(cli: Cli, out: &Output) -> Result<()> {
    if cli.network == Network::Bitcoin && !cli.allow_mainnet {
        bail!(
            "mainnet is disabled by default; this wallet is a learning project. \
             Pass --allow-mainnet (or WALLET_ALLOW_MAINNET=true) if you really mean it"
        );
    }
    if cli.network == Network::Bitcoin
        && !cli.encrypt
        && matches!(cli.command, Command::Init { .. } | Command::Restore { .. })
    {
        bail!("mainnet wallets must be encrypted; add --encrypt");
    }
    let node_mode = cli
        .node
        .unwrap_or_else(|| node::default_mode(cli.network, &cli.rpc));
    if node_mode == NodeMode::Polar && cli.network != Network::Regtest {
        bail!("Polar runs regtest only; drop --network or pick another --node");
    }
    // The local node's wallets; Polar's live apart, as it is another chain.
    let regtest = Wallets::new(None, cli.network);
    let wallets = if node_mode == NodeMode::Polar {
        Wallets::polar(None)
    } else {
        regtest.clone()
    };
    let (dir, name) = match &cli.datadir {
        Some(path) => (DataDir::at(path.clone()), None),
        None => {
            if let Some(moved) = wallets.migrate_legacy()? {
                out.note(&moved);
            }
            let name = wallets.resolve(cli.wallet.as_deref())?;
            (wallets.dir(&name), Some(name))
        }
    };
    let local = LocalNode {
        dir: cli.node_dir.clone().unwrap_or_else(|| match &cli.datadir {
            Some(path) => path.join("node"),
            None => regtest.root().join("node"),
        }),
        port: cli.node_port,
    };
    let target = Target {
        dir: dir.clone(),
        name: name.clone(),
        wallets: cli.datadir.is_none().then(|| wallets.clone()),
        pick: cli.datadir.is_none()
            && cli.wallet.is_none()
            && name.as_deref().is_some_and(|n| !wallets.exists(n)),
    };
    let open = |dir: &DataDir, network| open_wallet(dir, name.as_deref(), &wallets, network);
    let busy = activity::enabled(cli.json);
    match cli.command {
        Command::Wallets { action } => {
            if cli.datadir.is_some() {
                bail!("--datadir points at one wallet; named wallets do not apply");
            }
            match action.unwrap_or(WalletsAction::List) {
                WalletsAction::List => out.wallets(&wallets.list(), wallets.root()),
                WalletsAction::Create { name, words } => {
                    wallets::validate_name(&name)
                        .map_err(|e| anyhow::anyhow!("invalid wallet name {name:?}: {e}"))?;
                    let length = MnemonicLength::from_words(words).expect("validated by clap");
                    let phrase = generate_mnemonic(length).map_err(|e| anyhow::anyhow!(e))?;
                    let dir = wallets.dir(&name);
                    create(
                        &dir,
                        cli.network,
                        KeySource::mnemonic(phrase.as_str()),
                        None,
                        cli.encrypt,
                    )?;
                    out.created(&dir, cli.network, Some(phrase.as_str()), cli.encrypt);
                }
                WalletsAction::Use { name } => {
                    wallets.set_default(&name)?;
                    out.done(&format!("{name} is now the default wallet"));
                }
                WalletsAction::Rename { old, new } => {
                    wallets.rename(&old, &new)?;
                    out.done(&format!("renamed {old} to {new}"));
                }
                WalletsAction::Remove { name, yes, force } => {
                    if !wallets.exists(&name) {
                        bail!("no wallet named {name:?}; see `wallets`");
                    }
                    let held = wallets
                        .dir(&name)
                        .load_meta()?
                        .last_balance_sat
                        .unwrap_or(0);
                    if held > 0 && !force {
                        bail!(
                            "{name} still holds {} sat (as of its last sync); send it elsewhere first, or pass --force",
                            held
                        );
                    }
                    if !yes {
                        confirm(&format!("type {name} to delete it"), &name)?;
                    }
                    wallets.remove(&name)?;
                    out.done(&format!("removed {name}"));
                }
            }
        }
        Command::Node { action } => {
            if cli.network != Network::Regtest {
                bail!("the local node runs regtest only; drop --network");
            }
            let shared = node::Shared::new(local.dir.clone());
            match action {
                NodeAction::Start => {
                    let act = Activity::start(busy, "Starting the local regtest node…");
                    let started = shared.start(local.port, true)?;
                    act.done();
                    out.node_started(started, &shared);
                }
                NodeAction::Stop => {
                    let act = Activity::start(busy, "Stopping the local regtest node…");
                    let stopped = shared.stop()?;
                    act.done();
                    out.node_stopped(stopped);
                }
                NodeAction::Status => out.node_status(&shared.status(), &shared),
                NodeAction::Reset { yes } => {
                    if !yes {
                        confirm(
                            "type reset to delete the regtest chain (wallets are kept)",
                            "reset",
                        )?;
                    }
                    let act = Activity::start(busy, "Stopping and deleting the regtest chain…");
                    shared.reset()?;
                    act.done();
                    out.done("regtest chain deleted; mine again to get coins (wallets: sync to catch up)");
                }
            }
        }
        Command::Mine { blocks, to } => {
            if cli.network != Network::Regtest {
                bail!("mining only works on regtest");
            }
            let mut wallet = open(&dir, cli.network)?;
            let act = Activity::start(busy, "Opening wallet…");
            let conn = connect_node(node_mode, &cli.rpc, &local, cli.network, &act)?;
            let to = match to {
                Some(address) => address
                    .require_network(cli.network)
                    .context("--to is not a regtest address")?,
                None => wallet.new_address()?.address,
            };
            act.say(format!(
                "Mining {blocks} block{}…",
                if blocks == 1 { "" } else { "s" }
            ));
            conn.rpc()
                .generate_to_address(blocks, &to)
                .context("the node refused to mine (is it a regtest node?)")?;
            let report =
                sync_with(&mut wallet, &conn, &act, &dir).context("sync after mining failed")?;
            act.done();
            out.mined(blocks, &to, &report, &wallet.balance());
        }
        Command::Tui(args) => {
            return tui::run(args, cli.network, node_mode, target, cli.rpc.clone(), local);
        }
        Command::Init { words } => {
            let length = MnemonicLength::from_words(words).expect("validated by clap");
            let phrase = generate_mnemonic(length).map_err(|e| anyhow::anyhow!(e))?;
            create(
                &dir,
                cli.network,
                KeySource::mnemonic(phrase.as_str()),
                None,
                cli.encrypt,
            )?;
            out.created(&dir, cli.network, Some(phrase.as_str()), cli.encrypt);
        }
        Command::Restore {
            mnemonic,
            passphrase,
            birthday,
        } => {
            let keys = match &passphrase {
                Some(p) => KeySource::mnemonic_with_passphrase(mnemonic.as_str(), p.as_str()),
                None => KeySource::mnemonic(mnemonic.as_str()),
            };
            create(&dir, cli.network, keys, birthday, cli.encrypt)?;
            out.created(&dir, cli.network, None, cli.encrypt);
        }
        Command::Address { new } => {
            let mut wallet = open(&dir, cli.network)?;
            let info = if new {
                wallet.reveal_next_address()?
            } else {
                wallet.new_address()?
            };
            out.address(&info);
        }
        Command::Sync => {
            let mut wallet = open(&dir, cli.network)?;
            let act = Activity::start(busy, "Opening wallet…");
            let conn = connect_node(node_mode, &cli.rpc, &local, cli.network, &act)?;
            let report = sync_with(&mut wallet, &conn, &act, &dir).context("sync failed")?;
            act.done();
            out.sync(&report, &wallet.balance());
        }
        Command::Balance => out.balance(&open(&dir, cli.network)?.balance()),
        Command::Utxos(pages) => {
            let utxos = open(&dir, cli.network)?.list_utxos();
            out.utxos(&pages.apply(&utxos));
        }
        Command::History(pages) => {
            let txs = open(&dir, cli.network)?.transactions();
            out.history(&pages.apply(&txs));
        }
        Command::Send {
            address,
            amount,
            fee_rate,
            target,
            selection,
            dry_run,
        } => {
            let mut wallet = open(&dir, cli.network)?;
            let act = Activity::start(busy, "Opening wallet…");
            let conn = connect_node(node_mode, &cli.rpc, &local, cli.network, &act)?;
            sync_with(&mut wallet, &conn, &act, &dir).context("sync before send failed")?;
            let node = conn.rpc();

            let fee_rate = match fee_rate {
                Some(r) => fee_rate_from(r)?,
                None => {
                    act.say("Estimating the fee…");
                    estimate_fee(node, target, out)?
                }
            };
            let mut request = TxRequest::new(vec![Recipient::new(address, amount)], fee_rate);
            request.coin_selection = selection.map(Into::into);

            act.say("Selecting coins and signing…");
            let mut psbt = wallet.build_tx_with(&request)?;
            if wallet.sign(&mut psbt)? != SignOutcome::Finalized {
                bail!("transaction needs more signatures than this wallet holds");
            }
            let fee = psbt.fee().context("could not compute fee")?;
            let tx = psbt
                .extract_tx()
                .context("could not extract signed transaction")?;
            if dry_run {
                act.done();
                out.dry_run(&tx, fee, fee_rate);
            } else {
                act.say("Broadcasting…");
                let txid = node.broadcast(&tx)?;
                sync_with(&mut wallet, &conn, &act, &dir).context("sync after broadcast failed")?;
                act.done();
                out.sent(txid, fee, fee_rate);
            }
        }
        Command::Status { txid } => {
            let wallet = open(&dir, cli.network)?;
            match wallet.tx_status(txid) {
                Some(status) => out.status(txid, &status),
                None => bail!("transaction {txid} is not in this wallet; try `sync` first"),
            }
        }
        Command::Descriptors => {
            let wallet = open(&dir, cli.network)?;
            let (external, internal) = wallet.public_descriptors();
            out.descriptors(&external, &internal);
        }
        Command::ExportPsbt {
            address,
            amount,
            fee_rate,
            output,
        } => {
            let mut wallet = open(&dir, cli.network)?;
            let act = Activity::start(busy, "Opening wallet…");
            let conn = connect_node(node_mode, &cli.rpc, &local, cli.network, &act)?;
            sync_with(&mut wallet, &conn, &act, &dir).context("sync before export failed")?;

            let fee_rate = match fee_rate {
                Some(r) => fee_rate_from(r)?,
                None => {
                    act.say("Estimating the fee…");
                    estimate_fee(conn.rpc(), 6, out)?
                }
            };
            act.say("Building the PSBT…");
            let psbt = wallet.build_tx([Recipient::new(address, amount)], fee_rate)?;
            let psbt_bytes = psbt.serialize();
            std::fs::write(&output, &psbt_bytes)
                .with_context(|| format!("could not write {}", output.display()))?;
            act.done();
            out.psbt_exported(&output, psbt.fee().ok());
        }
        Command::SignPsbt { input, broadcast } => {
            let mut wallet = open(&dir, cli.network)?;
            let act = Activity::start(busy, "Opening wallet…");
            let conn = connect_node(node_mode, &cli.rpc, &local, cli.network, &act)?;
            sync_with(&mut wallet, &conn, &act, &dir).context("sync before sign failed")?;

            act.say("Signing…");
            let psbt_bytes = std::fs::read(&input)
                .with_context(|| format!("could not read {}", input.display()))?;
            let mut psbt =
                wallet::bitcoin::Psbt::deserialize(&psbt_bytes).context("could not parse PSBT")?;
            require_finalized(wallet.sign(&mut psbt)?)?;
            let fee = psbt.fee().context("could not compute fee")?;
            let tx = psbt.extract_tx().context("could not extract transaction")?;
            if broadcast {
                act.say("Broadcasting…");
                let txid = conn.rpc().broadcast(&tx)?;
                sync_with(&mut wallet, &conn, &act, &dir).context("sync after broadcast failed")?;
                act.done();
                out.sent(txid, fee, effective_fee_rate(fee, &tx));
            } else {
                act.done();
                out.signed_psbt(&tx);
            }
        }
        Command::BumpFee { txid, fee_rate } => {
            let mut wallet = open(&dir, cli.network)?;
            let act = Activity::start(busy, "Opening wallet…");
            let conn = connect_node(node_mode, &cli.rpc, &local, cli.network, &act)?;
            sync_with(&mut wallet, &conn, &act, &dir).context("sync before bump failed")?;

            act.say("Building and signing the replacement…");
            let new_rate = fee_rate_from(fee_rate)?;
            let mut psbt = wallet.bump_fee(txid, new_rate)?;
            require_finalized(wallet.sign(&mut psbt)?)?;
            let tx = psbt.extract_tx().context("could not extract transaction")?;
            act.say("Broadcasting…");
            let new_txid = conn.rpc().broadcast(&tx)?;
            sync_with(&mut wallet, &conn, &act, &dir).context("sync after bump failed")?;
            act.done();
            out.fee_bumped(txid, new_txid);
        }
    }
    Ok(())
}

fn create(
    dir: &DataDir,
    network: Network,
    keys: KeySource,
    birthday: Option<u32>,
    encrypt: bool,
) -> Result<()> {
    let password = if encrypt {
        Some(prompt_new_password()?)
    } else {
        None
    };
    session::create(
        dir,
        network,
        session::NewWallet {
            keys,
            birthday,
            password,
        },
    )?;
    return Ok(());
}

fn prompt_new_password() -> Result<Zeroizing<String>> {
    let password = Zeroizing::new(rpassword::prompt_password("encryption password: ")?);
    let confirm = Zeroizing::new(rpassword::prompt_password("confirm password: ")?);
    if password != confirm {
        bail!("passwords do not match");
    }
    if password.is_empty() {
        bail!("password must not be empty");
    }
    return Ok(password);
}

/// Open a wallet, explaining what exists when a named one is missing.
fn open_wallet(
    dir: &DataDir,
    name: Option<&str>,
    wallets: &Wallets,
    network: Network,
) -> Result<Wallet> {
    if let Some(name) = name
        && !session::exists(dir)
    {
        let others = wallets.names();
        if others.is_empty() {
            bail!("no wallet yet");
        }
        bail!(
            "no wallet named {name:?}; you have: {}. Pick one with -w <name> or `wallets use <name>`",
            others.join(", ")
        );
    }
    open_dir(dir, network)
}

/// Ask the user to type `expected` to confirm something destructive.
fn confirm(prompt: &str, expected: &str) -> Result<()> {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        bail!("refusing without confirmation; pass --yes");
    }
    eprint!("{prompt}: ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    if answer.trim() != expected {
        bail!("not confirmed; nothing was changed");
    }
    Ok(())
}

fn open_dir(dir: &DataDir, network: Network) -> Result<Wallet> {
    let keys = match session::stored_keys(dir)? {
        session::StoredKeys::Plain(keys) => Some(keys),
        session::StoredKeys::None => None,
        session::StoredKeys::Encrypted => {
            let password = Zeroizing::new(rpassword::prompt_password("wallet password: ")?);
            Some(session::unlock(dir, &password)?)
        }
    };
    session::open(dir, network, keys)
}

/// Connect to the node, telling the user what is happening meanwhile.
fn connect_node(
    mode: NodeMode,
    rpc: &RpcArgs,
    local: &LocalNode,
    network: Network,
    act: &Activity,
) -> Result<node::Node> {
    act.say(match mode {
        NodeMode::Local => "Starting or joining the local regtest node…".to_string(),
        _ => format!("Connecting to Bitcoin Core at {}…", rpc.url(network)),
    });
    let conn = node::connect(mode, rpc, local, network)?;
    act.say(format!("Connected to {}", conn.label()));
    Ok(conn)
}

/// Sync with a progress bar over the blocks still to fetch.
fn sync_with(
    wallet: &mut Wallet,
    conn: &node::Node,
    act: &Activity,
    dir: &DataDir,
) -> Result<SyncReport> {
    let local = wallet.tip().height;
    let tip = conn.rpc().tip()?.height;
    let missing = tip.saturating_sub(local);
    if missing > 0 {
        act.progress(u64::from(missing), "Syncing");
    } else {
        act.say("Checking the mempool…");
    }
    let progress = node::Progress::new(conn.rpc(), Duration::from_millis(50), |done| {
        act.set_position(u64::from(done));
    });
    let report = wallet.sync(&progress)?;
    // Remembered for `wallets`; a failure here only costs a stale number.
    let _ = dir.record_sync(wallet.balance().total().to_sat(), report.to.height);
    Ok(report)
}

fn require_finalized(outcome: SignOutcome) -> Result<()> {
    if outcome != SignOutcome::Finalized {
        bail!("transaction needs more signatures than this wallet holds");
    }
    Ok(())
}

/// The fee rate a signed transaction actually pays.
fn effective_fee_rate(fee: Amount, tx: &wallet::bitcoin::Transaction) -> FeeRate {
    FeeRate::from_sat_per_kwu(fee.to_sat() * 1000 / tx.weight().to_wu().max(1))
}

fn fee_rate_from(sat_vb: u64) -> Result<FeeRate> {
    FeeRate::from_sat_per_vb(sat_vb).context("fee rate is too large")
}

fn estimate_fee(node: &RpcClient, target: u16, out: &Output) -> Result<FeeRate> {
    match node.estimate_fee_rate(target) {
        Ok(rate) => Ok(rate),
        Err(FeeEstimateError::Unavailable { .. }) => {
            out.note(&format!(
                "node has no fee estimate yet, using {FALLBACK_FEE_RATE} sat/vB"
            ));
            fee_rate_from(FALLBACK_FEE_RATE)
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn networks_accept_common_and_core_names() {
        for (name, network) in [
            ("bitcoin", Network::Bitcoin),
            ("mainnet", Network::Bitcoin),
            ("main", Network::Bitcoin),
            ("testnet", Network::Testnet),
            ("test", Network::Testnet),
            ("testnet4", Network::Testnet4),
            ("signet", Network::Signet),
            ("REGTEST", Network::Regtest),
        ] {
            assert_eq!(parse_network(name), Ok(network), "{name}");
        }
        assert!(
            parse_network("moon")
                .unwrap_err()
                .contains("unknown network")
        );
    }

    #[test]
    fn word_counts_follow_bip39() {
        for words in [12, 15, 18, 21, 24] {
            assert_eq!(parse_word_count(&words.to_string()), Ok(words));
        }
        assert!(parse_word_count("13").is_err());
        assert!(parse_word_count("twelve").is_err());
    }
}
