//! An in-memory chain for deterministic tests without a node.
//!
//! Blocks are structurally valid (linked headers, correct merkle roots, BIP34
//! coinbases) but have no proof of work, which wallet engines do not check.

use std::cell::{Cell, RefCell};

use bitcoin::block::{Header, Version as BlockVersion};
use bitcoin::blockdata::constants::genesis_block;
use bitcoin::hashes::Hash;
use bitcoin::transaction::Version;
use bitcoin::{
    Amount, Block, BlockHash, FeeRate, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn,
    TxMerkleNode, TxOut, Txid, Witness, absolute,
};

use crate::error::{BroadcastError, FeeEstimateError, SourceError};
use crate::types::BlockId;
use crate::{BlockSource, Broadcaster, FeeEstimator};

/// A fake chain that implements every chain trait.
pub struct MockChain {
    blocks: Vec<Block>,
    /// Unconfirmed transactions with the "time" they were first seen.
    mempool: RefCell<Vec<(Transaction, u64)>>,
    /// Logical clock so later broadcasts look newer, as on a real node.
    clock: Cell<u64>,
    fee_rate: Option<FeeRate>,
    nonce: u32,
}

impl MockChain {
    /// A chain containing only `network`'s genesis block.
    pub fn new(network: Network) -> Self {
        Self {
            blocks: vec![genesis_block(network)],
            mempool: RefCell::new(Vec::new()),
            // Real unix time, like a node's mempool timestamps, so they
            // order correctly against wallet eviction times.
            clock: Cell::new(unix_now()),
            fee_rate: Some(FeeRate::from_sat_per_vb_u32(2)),
            nonce: 0,
        }
    }

    /// Height of the tip block.
    pub fn height(&self) -> u32 {
        (self.blocks.len() - 1) as u32
    }

    /// Mine a block containing `txs` plus everything in the mempool.
    pub fn mine(&mut self, txs: Vec<Transaction>) -> BlockId {
        let height = self.blocks.len() as u32;
        let prev = self.blocks.last().expect("genesis exists");
        let mut txdata = vec![coinbase(height, self.nonce)];
        txdata.extend(self.mempool.borrow_mut().drain(..).map(|(tx, _)| tx));
        txdata.extend(txs);
        let mut block = Block {
            header: Header {
                version: BlockVersion::TWO,
                prev_blockhash: prev.block_hash(),
                merkle_root: TxMerkleNode::all_zeros(),
                time: prev.header.time + 600,
                bits: prev.header.bits,
                nonce: self.nonce,
            },
            txdata,
        };
        block.header.merkle_root = block.compute_merkle_root().expect("non-empty block");
        self.nonce += 1;
        let hash = block.block_hash();
        self.blocks.push(block);
        BlockId { height, hash }
    }

    /// Mine `count` blocks holding only the mempool (empty after the first).
    pub fn mine_empty(&mut self, count: u32) -> BlockId {
        let mut last = self.tip().expect("mock never fails");
        for _ in 0..count {
            last = self.mine(Vec::new());
        }
        last
    }

    /// A transaction paying `amount` to `script` from a made-up outside input.
    pub fn payment(&mut self, script: ScriptBuf, amount: Amount) -> Transaction {
        self.nonce += 1;
        let funding = Txid::hash(&self.nonce.to_le_bytes());
        Transaction {
            version: Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(funding, 0),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::from_slice(&[[0u8; 72].as_slice(), [2u8; 33].as_slice()]),
            }],
            output: vec![TxOut {
                value: amount,
                script_pubkey: script,
            }],
        }
    }

    /// Pay `script` in a new block.
    pub fn fund(&mut self, script: ScriptBuf, amount: Amount) -> Txid {
        let tx = self.payment(script, amount);
        let txid = tx.compute_txid();
        self.mine(vec![tx]);
        txid
    }

    /// Pay `script` with a transaction that only sits in the mempool.
    pub fn fund_unconfirmed(&mut self, script: ScriptBuf, amount: Amount) -> Txid {
        let tx = self.payment(script, amount);
        let txid = tx.compute_txid();
        self.accept(tx);
        txid
    }

    /// Drop the top `depth` blocks, discarding their transactions.
    pub fn reorg(&mut self, depth: u32) {
        let keep = self.blocks.len() - depth as usize;
        assert!(keep >= 1, "cannot reorg away genesis");
        self.blocks.truncate(keep);
    }

    /// Add `tx` to the mempool, evicting anything that spends the same
    /// inputs. Like Bitcoin Core 28+ (full RBF), the newer transaction wins;
    /// fee rules are not checked because test inputs have no known value.
    fn accept(&self, tx: Transaction) {
        let spends: Vec<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
        // Strictly increasing even within one second.
        let seen = self.clock.get().max(unix_now());
        self.clock.set(seen + 1);
        let mut mempool = self.mempool.borrow_mut();
        mempool.retain(|(other, _)| {
            !other
                .input
                .iter()
                .any(|i| spends.contains(&i.previous_output))
        });
        mempool.push((tx, seen));
    }

    /// Txids currently in the mempool.
    pub fn mempool_txids(&self) -> Vec<Txid> {
        self.mempool
            .borrow()
            .iter()
            .map(|(tx, _)| tx.compute_txid())
            .collect()
    }

    /// Drop every mempool transaction, as if the node restarted.
    pub fn clear_mempool(&self) {
        self.mempool.borrow_mut().clear();
    }

    /// Make fee estimation return `rate`, or `Unavailable` when `None`.
    pub fn set_fee_rate(&mut self, rate: Option<FeeRate>) {
        self.fee_rate = rate;
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn coinbase(height: u32, nonce: u32) -> Transaction {
    let script_sig = bitcoin::script::Builder::new()
        .push_int(i64::from(height))
        .push_int(i64::from(nonce))
        .into_script();
    Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig,
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50 * 100_000_000),
            script_pubkey: ScriptBuf::new_op_return([0u8; 4]),
        }],
    }
}

impl BlockSource for MockChain {
    fn tip(&self) -> Result<BlockId, SourceError> {
        let block = self.blocks.last().expect("genesis exists");
        Ok(BlockId {
            height: self.height(),
            hash: block.block_hash(),
        })
    }

    fn block_hash(&self, height: u32) -> Result<BlockHash, SourceError> {
        self.blocks
            .get(height as usize)
            .map(Block::block_hash)
            .ok_or(SourceError::HeightNotFound(height))
    }

    fn block(&self, hash: &BlockHash) -> Result<Block, SourceError> {
        self.blocks
            .iter()
            .find(|b| b.block_hash() == *hash)
            .cloned()
            .ok_or(SourceError::BlockNotFound(*hash))
    }

    fn mempool(&self) -> Result<Vec<(Transaction, u64)>, SourceError> {
        Ok(self.mempool.borrow().clone())
    }
}

impl Broadcaster for MockChain {
    fn broadcast(&self, tx: &Transaction) -> Result<Txid, BroadcastError> {
        let txid = tx.compute_txid();
        if self.mempool_txids().contains(&txid) {
            return Err(BroadcastError::Rejected {
                reason: "txn-already-in-mempool".into(),
            });
        }
        self.accept(tx.clone());
        Ok(txid)
    }
}

impl FeeEstimator for MockChain {
    fn estimate_fee_rate(&self, target_blocks: u16) -> Result<FeeRate, FeeEstimateError> {
        self.fee_rate
            .ok_or(FeeEstimateError::Unavailable { target_blocks })
    }
}
