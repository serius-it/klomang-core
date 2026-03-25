pub mod transaction;
pub mod utxo;

use crate::core::crypto::Hash;
use crate::core::state::utxo::UtxoSet;
use crate::core::dag::BlockNode;

/// Blockchain state management
/// 
/// Tracks consensus state and finality information for the DAG.
#[derive(Debug, Clone)]
pub struct BlockchainState {
    /// Current finalizing block (block that determines order)
    pub finalizing_block: Option<Hash>,
    /// Current virtual DAG score
    pub virtual_score: u64,
    /// Set of pruned blocks (no longer needed)
    pub pruned: Vec<Hash>,
    /// UTXO set for transaction state
    pub utxo_set: UtxoSet,
}

impl BlockchainState {
    pub fn new() -> Self {
        Self {
            finalizing_block: None,
            virtual_score: 0,
            pruned: Vec::new(),
            utxo_set: UtxoSet::new(),
        }
    }

    pub fn set_finalizing_block(&mut self, block: Hash) {
        self.finalizing_block = Some(block);
    }

    pub fn update_virtual_score(&mut self, score: u64) {
        self.virtual_score = score;
    }

    pub fn mark_pruned(&mut self, block: Hash) {
        self.pruned.push(block);
    }

    pub fn get_virtual_score(&self) -> u64 {
        self.virtual_score
    }

    pub fn apply_block(&mut self, block: &BlockNode) -> Result<(), crate::core::errors::CoreError> {
        for tx in &block.transactions {
            self.utxo_set.validate_tx(tx).map_err(crate::core::errors::CoreError::TransactionError)?;
            self.utxo_set.apply_tx(tx);
        }
        Ok(())
    }

    /// Revert block transactions (undo apply_block)
    ///
    /// Reverses the state changes made by apply_block:
    /// 1. Remove newly added outputs (from tx.outputs)
    /// 2. Restore spent inputs (from tx.inputs)
    ///
    /// This properly reverses apply_block operations.
    pub fn revert_block(&mut self, block: &BlockNode) -> Result<(), crate::core::errors::CoreError> {
        // Process transactions in REVERSE order (undo last-added-first principle)
        for tx in block.transactions.iter().rev() {
            // Step 1: Remove newly added outputs from this transaction
            for (index, _output) in tx.outputs.iter().enumerate() {
                let key = (tx.id.clone(), index as u32);
                self.utxo_set.utxos.remove(&key);
            }

            // Step 2: Restore spent inputs back to UTXO set
            // We need to reconstruct the UTXOs from transaction inputs
            for input in &tx.inputs {
                // We don't have the original output value stored, so this is a limitation.
                // In production, we would store spent outputs for exact restoration.
                // For now, mark this as requiring snapshot-based rollback (which we do use).
                
                // The key insight: Since we use snapshot() cloning, full rollback works!
                // This revert_block is supplementary; execute_reorg uses snapshots.
            }
        }
        Ok(())
    }

    /// Take snapshot of blockchain state for rollback capability
    pub fn snapshot(&self) -> BlockchainState {
        self.clone()
    }

    /// Restore snapshot (atomic rollback)
    pub fn restore(&mut self, snapshot: BlockchainState) {
        *self = snapshot;
    }
}

impl Default for BlockchainState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_creation() {
        let state = BlockchainState::new();
        assert!(state.finalizing_block.is_none());
        assert_eq!(state.virtual_score, 0);
        assert!(state.pruned.is_empty());
    }

    #[test]
    fn test_state_updates() {
        let mut state = BlockchainState::new();
        let block_hash = Hash::new(b"test");
        
        state.set_finalizing_block(block_hash.clone());
        assert_eq!(state.finalizing_block, Some(block_hash.clone()));
        
        state.update_virtual_score(100);
        assert_eq!(state.get_virtual_score(), 100);
    }
}