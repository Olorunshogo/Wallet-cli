//! Block-based sync: pulls blocks from any [`BlockSource`] and feeds them to
//! BDK. Using our own trait instead of `bdk_bitcoind_rpc::Emitter` keeps the
//! chain layer engine-agnostic, so the v2 native engine can reuse it.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use bdk_wallet::bitcoin::constants::genesis_block;
use bdk_wallet::chain::ChainPosition;
use wallet_core::{BlockId, BlockSource, SourceError, SyncError, SyncReport, WalletEngine};

use crate::convert;
use crate::engine::BdkEngine;

/// Persist every this many blocks so a long first sync is resumable.
const PERSIST_EVERY: u32 = 500;

pub(crate) fn run(
    engine: &mut BdkEngine,
    source: &dyn BlockSource,
) -> Result<SyncReport, SyncError> {
    let network = engine.network();
    let genesis = genesis_block(network).block_hash();
    if source.block_hash(0)? != genesis {
        return Err(SyncError::WrongChain);
    }

    let local_tip = engine.wallet.latest_checkpoint();
    let from = convert::block_id(local_tip.block_id());

    // Walk our checkpoints down until one matches the backend's best chain.
    // Everything above that point was reorged out.
    let mut agree: Option<BlockId> = None;
    for cp in local_tip.iter() {
        match source.block_hash(cp.height()) {
            Ok(hash) if hash == cp.hash() => {
                agree = Some(convert::block_id(cp.block_id()));
                break;
            }
            Ok(_) | Err(SourceError::HeightNotFound(_)) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let agree = agree.ok_or(SyncError::WrongChain)?;
    // Checkpoints are sparse, so this is an upper bound on replaced blocks.
    let reorg_depth = if agree == from {
        0
    } else {
        from.height - agree.height
    };

    let tip = source.tip()?;
    let start = (agree.height + 1).max(engine.birthday.unwrap_or(0));
    let mut connected_to = agree;
    let mut blocks_applied = 0;

    for height in start..=tip.height {
        let hash = source.block_hash(height)?;
        let block = source.block(&hash)?;
        engine
            .wallet
            .apply_block_connected_to(&block, height, convert::bdk_block_id(connected_to))
            .map_err(|e| SyncError::ApplyBlock {
                height,
                source: Box::new(e),
            })?;
        connected_to = BlockId { height, hash };
        blocks_applied += 1;
        if blocks_applied % PERSIST_EVERY == 0 {
            engine
                .persist()
                .map_err(|e| SyncError::Persist(Box::new(e)))?;
        }
    }

    let mempool = source.mempool()?;
    let mempool_txs = mempool.len();
    let in_mempool: HashSet<_> = mempool.iter().map(|(tx, _)| tx.compute_txid()).collect();
    engine.wallet.apply_unconfirmed_txs(mempool);

    // Unconfirmed wallet txs the node no longer has were dropped or replaced.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let evicted: Vec<_> = engine
        .wallet
        .transactions()
        .filter(|t| matches!(t.chain_position, ChainPosition::Unconfirmed { .. }))
        .map(|t| t.tx_node.txid)
        .filter(|txid| !in_mempool.contains(txid))
        .map(|txid| (txid, now))
        .collect();
    engine.wallet.apply_evicted_txs(evicted);

    engine
        .persist()
        .map_err(|e| SyncError::Persist(Box::new(e)))?;

    Ok(SyncReport {
        from,
        to: engine.tip(),
        blocks_applied,
        reorg_depth,
        mempool_txs,
    })
}
