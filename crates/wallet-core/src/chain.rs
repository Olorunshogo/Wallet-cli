use bitcoin::{Block, BlockHash, FeeRate, Transaction, Txid};

use crate::error::{BroadcastError, FeeEstimateError, SourceError};
use crate::types::BlockId;

// === Chain traits
//
// Split by capability so a backend only implements what it can do: a
// read-only block source does not have to support broadcasting.

/// A source of full blocks and mempool transactions.
///
/// Engines consume blocks rather than per-script history, so any backend that
/// can hand out blocks by height (Bitcoin Core RPC, a local block file reader,
/// a test fixture) can drive a sync.
pub trait BlockSource {
    /// Current best block.
    fn tip(&self) -> Result<BlockId, SourceError>;

    /// Hash of the block at `height` on the best chain.
    fn block_hash(&self, height: u32) -> Result<BlockHash, SourceError>;

    /// Full block by hash.
    fn block(&self, hash: &BlockHash) -> Result<Block, SourceError>;

    /// Unconfirmed transactions, each with a unix timestamp of when it was
    /// last seen. Used by engines to resolve conflicts between replacements.
    fn mempool(&self) -> Result<Vec<(Transaction, u64)>, SourceError>;
}

/// Something that can relay a signed transaction to the network.
pub trait Broadcaster {
    /// Send `tx` to the network and return its txid.
    fn broadcast(&self, tx: &Transaction) -> Result<Txid, BroadcastError>;
}

/// Something that can suggest a fee rate for a confirmation target.
pub trait FeeEstimator {
    /// Fee rate likely to confirm within `target_blocks` blocks.
    fn estimate_fee_rate(&self, target_blocks: u16) -> Result<FeeRate, FeeEstimateError>;
}

// Blanket impls so callers can pass `&T`, `Box<T>` or `Box<dyn Trait>`.

impl<T: BlockSource + ?Sized> BlockSource for &T {
    fn tip(&self) -> Result<BlockId, SourceError> {
        (**self).tip()
    }
    fn block_hash(&self, height: u32) -> Result<BlockHash, SourceError> {
        (**self).block_hash(height)
    }
    fn block(&self, hash: &BlockHash) -> Result<Block, SourceError> {
        (**self).block(hash)
    }
    fn mempool(&self) -> Result<Vec<(Transaction, u64)>, SourceError> {
        (**self).mempool()
    }
}

impl<T: BlockSource + ?Sized> BlockSource for Box<T> {
    fn tip(&self) -> Result<BlockId, SourceError> {
        (**self).tip()
    }
    fn block_hash(&self, height: u32) -> Result<BlockHash, SourceError> {
        (**self).block_hash(height)
    }
    fn block(&self, hash: &BlockHash) -> Result<Block, SourceError> {
        (**self).block(hash)
    }
    fn mempool(&self) -> Result<Vec<(Transaction, u64)>, SourceError> {
        (**self).mempool()
    }
}

impl<T: Broadcaster + ?Sized> Broadcaster for &T {
    fn broadcast(&self, tx: &Transaction) -> Result<Txid, BroadcastError> {
        (**self).broadcast(tx)
    }
}

impl<T: Broadcaster + ?Sized> Broadcaster for Box<T> {
    fn broadcast(&self, tx: &Transaction) -> Result<Txid, BroadcastError> {
        (**self).broadcast(tx)
    }
}

impl<T: FeeEstimator + ?Sized> FeeEstimator for &T {
    fn estimate_fee_rate(&self, target_blocks: u16) -> Result<FeeRate, FeeEstimateError> {
        (**self).estimate_fee_rate(target_blocks)
    }
}

impl<T: FeeEstimator + ?Sized> FeeEstimator for Box<T> {
    fn estimate_fee_rate(&self, target_blocks: u16) -> Result<FeeRate, FeeEstimateError> {
        (**self).estimate_fee_rate(target_blocks)
    }
}
