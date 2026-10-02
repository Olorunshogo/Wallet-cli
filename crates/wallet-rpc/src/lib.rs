//! Bitcoin Core JSON-RPC backend.
//!
//! Implements [`BlockSource`], [`Broadcaster`] and [`FeeEstimator`] from
//! `wallet-core`. Works with any engine, and needs no wallet loaded in Core and no `txindex`: blocks are fetched raw and filtered by the engine.

#![warn(missing_docs)]
// Explicit returns are a project convention; clippy's needless_return lint
// disagrees, so allow it crate-wide.
#![allow(clippy::needless_return)]

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bitcoincore_rpc::jsonrpc::Client as JsonRpcClient;
use bitcoincore_rpc::jsonrpc::error::RpcError;
use bitcoincore_rpc::jsonrpc::simple_http::SimpleHttpTransport;
use bitcoincore_rpc::{Auth, Client, RpcApi};
use wallet_core::bitcoin::{Block, BlockHash, FeeRate, Transaction, Txid};
use wallet_core::{
    BlockId, BlockSource, BroadcastError, Broadcaster, FeeEstimateError, FeeEstimator, SourceError,
};

// === Bitcoin Core RPC error codes we translate into specific variants.
const RPC_INVALID_PARAMETER: i32 = -8;
const RPC_INVALID_ADDRESS_OR_KEY: i32 = -5;
const RPC_VERIFY_ERROR: i32 = -25;
const RPC_VERIFY_REJECTED: i32 = -26;
const RPC_VERIFY_ALREADY_IN_CHAIN: i32 = -27;

/// How to authenticate with the node.
#[derive(Debug, Clone)]
pub enum RpcAuth {
    /// Path to the node's `.cookie` file (the default for a local node).
    Cookie(PathBuf),
    /// `rpcuser` / `rpcpassword` from `bitcoin.conf`.
    UserPass {
        /// RPC user name.
        user: String,
        /// RPC password.
        pass: String,
    },
}

/// Timeout applied to each RPC call unless [`RpcClient::with_timeout`] is used.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// A Bitcoin Core node reached over JSON-RPC.
pub struct RpcClient {
    client: Client,
}

impl RpcClient {
    /// Connect with [`DEFAULT_TIMEOUT`] per call.
    pub fn new(url: &str, auth: RpcAuth) -> Result<Self, SourceError> {
        return Self::with_timeout(url, auth, DEFAULT_TIMEOUT);
    }

    /// Connect with a custom per-call timeout. A call that gets no response
    /// within `timeout` fails with [`SourceError::Connection`] instead of
    /// hanging, and read calls are retried a few times before giving up.
    pub fn with_timeout(url: &str, auth: RpcAuth, timeout: Duration) -> Result<Self, SourceError> {
        let auth = match auth {
            RpcAuth::Cookie(path) => Auth::CookieFile(path),
            RpcAuth::UserPass { user, pass } => Auth::UserPass(user, pass),
        };
        // Reads the cookie file now, so a missing or unreadable cookie fails here.
        let (user, pass) = auth
            .get_user_pass()
            .map_err(|e| SourceError::Connection(Box::new(e)))?;
        let mut builder = SimpleHttpTransport::builder()
            .url(url)
            .map_err(|e| SourceError::Connection(Box::new(e)))?
            .timeout(timeout);
        if let Some(user) = user {
            builder = builder.auth(user, pass);
        }
        let client = Client::from_jsonrpc(JsonRpcClient::with_transport(builder.build()));
        return Ok(Self { client });
    }

    fn with_retry<T>(
        &self,
        operation: &str,
        f: impl Fn() -> Result<T, SourceError>,
    ) -> Result<T, SourceError> {
        let mut attempts = 0u32;
        loop {
            match f() {
                Ok(v) => return Ok(v),
                Err(e) if attempts < 3 && is_transient(&e) => {
                    attempts += 1;
                    let delay = Duration::from_millis(100 * attempts as u64);
                    tracing::warn!(operation, attempts, ?delay, "retrying RPC call");
                    std::thread::sleep(delay);
                }
                Err(e) => return Err(e),
            }
        }
    }
}

fn is_transient(err: &SourceError) -> bool {
    matches!(err, SourceError::Connection(_))
}

fn rpc_code(err: &bitcoincore_rpc::Error) -> Option<&RpcError> {
    match err {
        bitcoincore_rpc::Error::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Rpc(e)) => Some(e),
        _ => None,
    }
}

fn source_error(err: bitcoincore_rpc::Error) -> SourceError {
    match err {
        bitcoincore_rpc::Error::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Rpc(_)) => {
            SourceError::InvalidResponse(Box::new(err))
        }
        bitcoincore_rpc::Error::JsonRpc(_) | bitcoincore_rpc::Error::Io(_) => {
            SourceError::Connection(Box::new(err))
        }
        other => SourceError::InvalidResponse(Box::new(other)),
    }
}

// === Regtest helpers

impl RpcClient {
    /// Mine `blocks` blocks paying their rewards to `to`. **Regtest only**:
    /// on other networks the node refuses. Useful for tests and demos.
    pub fn generate_to_address(
        &self,
        blocks: u32,
        to: &wallet_core::bitcoin::Address,
    ) -> Result<Vec<BlockHash>, SourceError> {
        return self
            .client
            .generate_to_address(u64::from(blocks), to)
            .map_err(source_error);
    }

    /// Ask the node to shut down. Use for nodes your program manages.
    pub fn stop(&self) -> Result<(), SourceError> {
        self.client.stop().map_err(source_error)?;
        return Ok(());
    }
}

// === BlockSource

impl BlockSource for RpcClient {
    fn tip(&self) -> Result<BlockId, SourceError> {
        self.with_retry("tip", || {
            let info = self.client.get_blockchain_info().map_err(source_error)?;
            return Ok(BlockId {
                height: info.blocks as u32,
                hash: info.best_block_hash,
            });
        })
    }

    fn block_hash(&self, height: u32) -> Result<BlockHash, SourceError> {
        self.with_retry("block_hash", || {
            return self
                .client
                .get_block_hash(u64::from(height))
                .map_err(|e| match rpc_code(&e) {
                    Some(rpc) if rpc.code == RPC_INVALID_PARAMETER => {
                        SourceError::HeightNotFound(height)
                    }
                    _ => source_error(e),
                });
        })
    }

    fn block(&self, hash: &BlockHash) -> Result<Block, SourceError> {
        self.with_retry("block", || {
            // `getblock` at verbosity 0 returns raw bytes; decoding locally is much
            // faster than the verbose JSON form.
            return self.client.get_block(hash).map_err(|e| match rpc_code(&e) {
                Some(rpc) if rpc.code == RPC_INVALID_ADDRESS_OR_KEY => {
                    SourceError::BlockNotFound(*hash)
                }
                _ => source_error(e),
            });
        })
    }

    fn mempool(&self) -> Result<Vec<(Transaction, u64)>, SourceError> {
        self.with_retry("mempool", || {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default();
            let txids = self.client.get_raw_mempool().map_err(source_error)?;
            let mut txs = Vec::with_capacity(txids.len());
            for txid in txids {
                match self.client.get_raw_transaction(&txid, None) {
                    Ok(tx) => txs.push((tx, now)),
                    // Mined or evicted between the two calls; the next sync sees it.
                    Err(e)
                        if rpc_code(&e).is_some_and(|r| r.code == RPC_INVALID_ADDRESS_OR_KEY) => {}
                    Err(e) => return Err(source_error(e)),
                }
            }
            return Ok(txs);
        })
    }
}

// === Broadcaster

impl Broadcaster for RpcClient {
    fn broadcast(&self, tx: &Transaction) -> Result<Txid, BroadcastError> {
        let txid = tx.compute_txid();
        let span = tracing::info_span!("rpc.broadcast", txid = %txid);
        let _guard = span.enter();
        return self
            .client
            .send_raw_transaction(tx)
            .map_err(|e| match rpc_code(&e) {
                Some(rpc)
                    if matches!(
                        rpc.code,
                        RPC_VERIFY_ERROR | RPC_VERIFY_REJECTED | RPC_VERIFY_ALREADY_IN_CHAIN
                    ) =>
                {
                    BroadcastError::Rejected {
                        reason: rpc.message.clone(),
                    }
                }
                _ => BroadcastError::Connection(Box::new(e)),
            });
    }
}

// === FeeEstimator

impl FeeEstimator for RpcClient {
    fn estimate_fee_rate(&self, target_blocks: u16) -> Result<FeeRate, FeeEstimateError> {
        let span = tracing::info_span!("rpc.estimate_fee_rate", target_blocks);
        let _guard = span.enter();
        let estimate = self
            .client
            .estimate_smart_fee(target_blocks, None)
            .map_err(|e| FeeEstimateError::Connection(Box::new(e)))?;
        // Core returns BTC/kvB; convert to sat/kwu (1 vB = 4 wu).
        let per_kvb = estimate
            .fee_rate
            .ok_or(FeeEstimateError::Unavailable { target_blocks })?;
        return Ok(FeeRate::from_sat_per_kwu(per_kvb.to_sat() / 4));
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::time::Instant;

    use super::*;

    #[test]
    fn unresponsive_node_times_out_instead_of_hanging() {
        // Accepts connections but never answers.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let _accept = std::thread::spawn(move || {
            let _held: Vec<_> = listener.incoming().collect();
        });

        let auth = RpcAuth::UserPass {
            user: "u".into(),
            pass: "p".into(),
        };
        let rpc = RpcClient::with_timeout(&url, auth, Duration::from_millis(200)).unwrap();

        let started = Instant::now();
        let result = rpc.tip();
        assert!(
            matches!(result, Err(SourceError::Connection(_))),
            "{result:?}"
        );
        // 4 attempts x 200 ms plus backoff; far below the 30 s default.
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn missing_cookie_file_fails_at_construction() {
        let auth = RpcAuth::Cookie("/definitely/not/here/.cookie".into());
        let result = RpcClient::new("http://127.0.0.1:1", auth);
        assert!(matches!(result, Err(SourceError::Connection(_))));
    }
}
