//! Chain backends the TUI can run against.
//!
//! The worker only needs something that is a [`BlockSource`],
//! [`Broadcaster`] and [`FeeEstimator`] at once, plus an optional [`Miner`]
//! for the demo. Bitcoin Core, the embedded demo node and test doubles all
//! plug in here, which is the library's pluggable-backend design at work.

use wallet::bitcoin::{Address, Network};
use wallet::{BlockSource, BoxError, Broadcaster, FeeEstimator};

use crate::config::RpcArgs;

/// Everything the worker needs from a chain backend.
pub trait ChainBackend: BlockSource + Broadcaster + FeeEstimator {}

impl<T: BlockSource + Broadcaster + FeeEstimator + ?Sized> ChainBackend for T {}

/// Produces blocks on demand. Only regtest backends implement it.
pub trait Miner {
    /// Mine `blocks` blocks paying their rewards to `to`.
    fn mine(&self, blocks: u32, to: &Address) -> Result<(), BoxError>;
}

/// A connected backend.
pub struct Chain {
    /// Serves blocks, broadcasts and fee estimates.
    pub backend: Box<dyn ChainBackend>,
    /// Present only on regtest backends.
    pub miner: Option<Box<dyn Miner>>,
    /// Shown in the header, e.g. "Bitcoin Core at http://127.0.0.1:18443".
    pub label: String,
    /// Never read: holding it keeps a node the TUI started alive, and
    /// dropping it (when the TUI quits) lets that node stop.
    pub _keepalive: Option<Box<dyn std::any::Any + Send>>,
}

/// Connects to a [`Chain`] on the worker thread (backends need not be
/// `Send`). Called again on every reconnect attempt, so it must be repeatable.
pub type ChainFactory = Box<dyn FnMut() -> Result<Chain, BoxError> + Send>;

pub use crate::node::NodeMode;

/// Bitcoin Core over JSON-RPC, using the CLI's `--rpc-*` settings. This is
/// also how Polar's regtest `bitcoind` is reached. On regtest the same node
/// mines, so the faucet and mining keys work there too.
pub fn rpc(args: RpcArgs, network: Network) -> ChainFactory {
    reachable(args, network, "Bitcoin Core", |url, network, e| {
        unreachable_hint(url, network, e)
    })
}

/// Polar's regtest bitcoind, with Polar's default RPC settings unless the
/// `--rpc-*` settings override them.
pub fn polar(args: RpcArgs) -> ChainFactory {
    reachable(args.polar(), Network::Regtest, "Polar node", |url, _, e| {
        crate::node::polar_unreachable(url, &crate::tui::format::chain(e))
    })
}

/// A Bitcoin Core RPC backend that must answer before it counts as
/// connected; `explain` turns a failure into what the user should do.
fn reachable(
    args: RpcArgs,
    network: Network,
    name: &'static str,
    explain: fn(&str, Network, &dyn std::error::Error) -> String,
) -> ChainFactory {
    Box::new(move || {
        let url = args.url(network);
        let reach = || -> Result<Chain, BoxError> {
            let client = args.connect(network).map_err(BoxError::from)?;
            client.tip()?;
            let miner: Option<Box<dyn Miner>> = (network == Network::Regtest)
                .then(|| args.connect(network))
                .transpose()?
                .map(|c| Box::new(RpcMiner(c)) as Box<dyn Miner>);
            Ok(Chain {
                backend: Box::new(client),
                miner,
                label: format!("{name} at {url}"),
                _keepalive: None,
            })
        };
        reach().map_err(|e| explain(&url, network, e.as_ref()).into())
    })
}

/// Why an external node could not be reached, and what to do about it.
fn unreachable_hint(url: &str, network: Network, err: &dyn std::error::Error) -> String {
    let cause = crate::tui::format::chain(err);
    let fix = if network == Network::Regtest {
        "start bitcoind -regtest, use --node local, or for Polar set \
         WALLET_RPC_URL, WALLET_RPC_USER and WALLET_RPC_PASS"
    } else {
        "start your node, or set WALLET_RPC_URL and its cookie or user/password"
    };
    format!("no Bitcoin Core at {url} ({cause}); {fix}")
}

/// Mines through Bitcoin Core's `generatetoaddress` (regtest only).
struct RpcMiner(wallet::rpc::RpcClient);

impl Miner for RpcMiner {
    fn mine(&self, blocks: u32, to: &Address) -> Result<(), BoxError> {
        self.0.generate_to_address(blocks, to)?;
        Ok(())
    }
}

/// The shared local regtest node, started on demand. The worker holds its
/// lease, so quitting the TUI stops a node nobody else is using.
pub fn shared(local: crate::node::LocalNode) -> ChainFactory {
    Box::new(move || {
        let node = crate::node::connect(
            NodeMode::Local,
            &crate::config::RpcArgs::default(),
            &local,
            Network::Regtest,
        )?;
        let miner = crate::node::Shared::new(local.dir.clone()).client()?;
        let (client, label, keepalive) = node.into_parts();
        Ok(Chain {
            backend: Box::new(client),
            miner: Some(Box::new(RpcMiner(miner))),
            label,
            _keepalive: keepalive,
        })
    })
}

/// A throwaway regtest node for `--demo`, stopped when the TUI exits.
#[cfg(feature = "local-node")]
pub fn throwaway() -> ChainFactory {
    Box::new(|| {
        let (node, client) = crate::node::start_throwaway()?;
        let miner = RpcMiner(wallet::rpc::RpcClient::new(
            &node.rpc_url(),
            wallet::rpc::RpcAuth::Cookie(node.params.cookie_file.clone()),
        )?);
        Ok(Chain {
            backend: Box::new(client),
            miner: Some(Box::new(miner)),
            label: "throwaway regtest node".into(),
            _keepalive: Some(Box::new(node)),
        })
    })
}

pub use crate::node::Progress;

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::time::Duration;

    use wallet::testkit::MockChain;

    use super::*;

    #[test]
    fn progress_counts_blocks_and_throttles_reports() {
        let mut chain = MockChain::new(Network::Regtest);
        chain.mine_empty(5);
        let reports = RefCell::new(Vec::new());
        let progress = Progress::new(&chain, Duration::from_secs(60), |n| {
            reports.borrow_mut().push(n)
        });
        for h in 1..=5 {
            progress.block(&progress.block_hash(h).unwrap()).unwrap();
        }
        assert_eq!(progress.fetched(), 5);
        assert_eq!(
            *reports.borrow(),
            vec![1],
            "throttled to one report per minute"
        );
    }
}
