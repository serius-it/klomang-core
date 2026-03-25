/// DAG Reorganization Mechanism
///
/// Handles chain reorganization when a better (higher blue_score) chain is discovered.
/// Ensures atomic state transitions with no partial updates.
///
/// Operations:
/// 1. Detect better chain by comparing blue_score
/// 2. Find common ancestor between old and new chains
/// 3. Rollback current chain state (revert UTXO changes)
/// 4. Apply new chain (update UTXO and state)
/// 5. Update virtual block, selected chain, and accepted transactions

use crate::core::state::transaction::Transaction;

/// Temporary buffer for reverted transactions during reorg
///
/// Prevents borrow conflicts by decoupling mempool operations from consensus/state updates.
/// Transactions are collected during reorg and reinserted into mempool after state commit.
#[derive(Debug, Clone)]
pub struct ReorgTxBuffer {
    /// Reverted transactions from disconnected blocks
    pub reverted: Vec<Transaction>,
}

impl ReorgTxBuffer {
    /// Create empty buffer
    pub fn new() -> Self {
        Self {
            reverted: Vec::new(),
        }
    }

    /// Create buffer with transactions
    pub fn with_transactions(txs: Vec<Transaction>) -> Self {
        Self { reverted: txs }
    }

    /// Check if buffer has transactions
    pub fn is_empty(&self) -> bool {
        self.reverted.is_empty()
    }

    /// Get number of transactions
    pub fn len(&self) -> usize {
        self.reverted.len()
    }
}

pub mod ancestor;
pub mod apply;
pub mod detect;
pub mod execute;
pub mod path;
pub mod rollback;
pub mod snapshot;
pub mod validate;

#[cfg(test)]
mod tests;

pub use ancestor::find_common_ancestor;
pub use apply::{apply_blocks, collect_reverted_transactions, collect_reverted_transactions_from_reorg};
pub use detect::{check_and_prepare_reorg, detect_reorg};
pub use execute::{execute_reorg, execute_reorg_with_recovery, execute_reorg_with_buffer};
pub use path::{calculate_path_length, collect_chain};
pub use rollback::rollback_blocks;
pub use snapshot::ReorgState;
pub use validate::validate_reorg;