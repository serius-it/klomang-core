use crate::core::dag::{Dag, BlockNode};
use crate::core::consensus::GhostDag;
use crate::core::consensus::ghostdag::VirtualBlock;
use crate::core::crypto::Hash;
use crate::core::errors::CoreError;
use crate::core::config::Config;
use crate::core::state::BlockchainState;
use crate::core::state::transaction::Transaction;
use crate::core::storage::Storage;
use std::collections::HashSet;

use super::block_pipeline;
use super::mempool;
use super::ordering;

/// BlockDAG Engine - Core consensus engine for blockchain
///
/// Manages the Directed Acyclic Graph (DAG) of blocks and applies
/// GHOSTDAG consensus algorithm for order and finality.
pub struct Engine {
    dag: Dag,
    ghostdag: GhostDag,
    config: Config,
    state: BlockchainState,
    storage: Box<dyn Storage>,
    mempool: mempool::Mempool,
    genesis_hash: Option<Hash>,
    finalized_tip: Option<Hash>,
}

impl Engine {
    pub fn new() -> Self {
        let config = Config::default();
        Self::with_config(config)
    }

    pub fn with_config(config: Config) -> Self {
        Self {
            dag: Dag::new(),
            ghostdag: GhostDag::new(config.k),
            config,
            state: BlockchainState::new(),
            storage: Box::new(crate::core::storage::MemoryStorage::new()),
            mempool: mempool::Mempool::new(),
            genesis_hash: None,
            finalized_tip: None,
        }
    }

    pub fn with_storage(mut self, storage: Box<dyn Storage>) -> Self {
        self.storage = storage;
        self
    }

    /// Adds a block to the core engine and updates DAG and consensus state.
    /// Delegates to process_block for the actual pipeline logic.
    pub fn add_block(&mut self, block: BlockNode) -> Result<(), CoreError> {
        block_pipeline::process_block(self, block)
    }

    /// Add genesis block as first entry
    pub fn add_genesis(&mut self, block: BlockNode) -> Result<(), CoreError> {
        if !block.parents.is_empty() {
            return Err(CoreError::InvalidParent);
        }

        self.add_block(block)
    }

    /// Get block by hash through engine interface
    pub fn get_block(&self, hash: &Hash) -> Option<&BlockNode> {
        self.dag.get_block(hash)
    }

    /// Submit a transaction to the engine mempool
    pub fn submit_tx(&mut self, tx: Transaction) -> Result<(), CoreError> {
        self.mempool.submit_tx(tx)
    }

    /// Remove confirmed transactions from mempool.
    pub fn remove_confirmed(&mut self, tx_ids: &[Hash]) -> Result<(), CoreError> {
        self.mempool.remove_confirmed(tx_ids)
    }

    /// Select transactions for block creation from mempool.
    /// Uses fee-rate priority with DAG-aware topological ordering.
    pub fn select_txs_for_block(&self) -> Result<Vec<Transaction>, CoreError> {
        self.mempool.select_txs_for_block()
    }

    /// Select limited number of transactions for block creation
    pub fn select_txs_limited(&self, max_count: usize) -> Result<Vec<Transaction>, CoreError> {
        self.mempool.select_txs_limited(max_count)
    }

    /// Get mempool statistics
    pub fn get_mempool_stats(&self) -> Result<mempool::MempoolStats, CoreError> {
        self.mempool.get_stats()
    }

    pub fn get_state(&self) -> &BlockchainState {
        &self.state
    }

    pub fn get_tips(&self) -> Vec<Hash> {
        self.dag.get_tips().iter().cloned().collect()
    }

    pub fn get_genesis(&self) -> Option<&Hash> {
        self.genesis_hash.as_ref()
    }

    pub fn get_block_count(&self) -> usize {
        self.dag.get_block_count()
    }

    pub fn block_exists(&self, hash: &Hash) -> bool {
        self.dag.block_exists(hash)
    }

    pub fn get_ordering(&self) -> Vec<Hash> {
        ordering::get_ordered_blocks(&self.dag, &self.ghostdag)
    }

    pub fn get_selected_chain(&self) -> Vec<Hash> {
        self.ghostdag.get_virtual_selected_chain(&self.dag)
    }

    pub fn get_virtual_block(&self) -> VirtualBlock {
        self.ghostdag.build_virtual_block(&self.dag)
    }

    pub fn get_finalized_tip(&self) -> Option<&Hash> {
        self.finalized_tip.as_ref()
    }

    pub fn get_ancestors(&self, hash: &Hash) -> HashSet<Hash> {
        self.dag.get_ancestors(hash).into_iter().collect()
    }

    pub fn get_descendants(&self, hash: &Hash) -> HashSet<Hash> {
        self.dag.get_descendants(hash).into_iter().collect()
    }

    pub fn get_blue_set(&self, hash: &Hash) -> HashSet<Hash> {
        self.ghostdag.get_blue_set(&self.dag, hash)
    }

    pub fn get_red_set(&self, hash: &Hash) -> HashSet<Hash> {
        self.ghostdag.get_red_set(&self.dag, hash)
    }

    pub fn get_k(&self) -> usize {
        self.config.k
    }

    /// Update finality based on selected chain
    pub fn update_finality(&mut self) {
        let selected_chain = self.get_selected_chain();
        if selected_chain.len() > self.config.finality_depth {
            let finalized_index = selected_chain.len() - self.config.finality_depth;
            self.finalized_tip = Some(selected_chain[finalized_index].clone());
        }
    }

    /// Prune old blocks that are not ancestors of finalized tip and not in recent window
    pub fn prune(&mut self) {
        let finalized_tip = match self.finalized_tip.as_ref() {
            Some(hash) => hash.clone(),
            None => return,
        };

        let all_blocks: Vec<Hash> = self.dag.get_all_hashes().into_iter().collect();
        let recent_window = 2 * self.config.finality_depth;

        for block_hash in all_blocks {
            if self.dag.is_ancestor(&finalized_tip, &block_hash) {
                continue; // Keep ancestors of finalized tip
            }

            // Check if in recent window (approximate by blue score distance)
            if let Some(block) = self.dag.get_block(&block_hash) {
                if let Some(finalized_block) = self.dag.get_block(&finalized_tip) {
                    if block.blue_score >= finalized_block.blue_score.saturating_sub(recent_window as u64) {
                        continue; // Keep recent blocks
                    }
                }
            }

            // Prune the block
            self.dag.blocks.remove(&block_hash);
            self.storage.delete_block(&block_hash);
        }
    }

    /// Save a block to storage
    pub fn save_block(&mut self, block: BlockNode) {
        self.storage.put_block(block);
    }

    /// Load a block from storage
    pub fn load_block(&self, id: &Hash) -> Option<BlockNode> {
        self.storage.get_block(id)
    }

    /// Calculate difficulty for new block based on DAA
    pub fn calculate_difficulty(&self, block_timestamp: u64) -> u64 {
        use crate::core::daa::difficulty::Daa;
        let daa = Daa::new(self.config.target_block_time, 10);
        daa.calculate_next_difficulty(self.dag(), block_timestamp)
    }

    /// Mine a block by finding a valid nonce for Proof of Work
    pub fn mine_block(mut block: BlockNode) -> Result<BlockNode, CoreError> {
        let header = Self::serialize_header(&block);
        let target = Self::calculate_target_u64(block.difficulty);
        if let Some(nonce) = crate::core::pow::mine_block(&header, target) {
            block.nonce = nonce;
            Ok(block)
        } else {
            Err(CoreError::ConsensusError)
        }
    }

    /// Check if block's PoW is valid
    pub fn pow_valid(block: &BlockNode) -> bool {
        let header = Self::serialize_header(block);
        let hash = crate::core::pow::calculate_hash(&header);
        let target = Self::calculate_target_u64(block.difficulty);
        crate::core::pow::is_valid_pow(&hash, target)
    }

    fn serialize_header(block: &BlockNode) -> Vec<u8> {
        let mut data = Vec::new();
        // Sort parents for deterministic order
        let mut parents: Vec<_> = block.parents.iter().collect();
        parents.sort();
        for parent in parents {
            data.extend_from_slice(parent.as_bytes());
        }
        data.extend_from_slice(&block.timestamp.to_le_bytes());
        data.extend_from_slice(&block.difficulty.to_le_bytes());
        data.extend_from_slice(&block.nonce.to_le_bytes());
        data
    }

    fn calculate_target_u64(difficulty: u64) -> u64 {
        if difficulty == 0 {
            u64::MAX
        } else {
            u64::MAX / difficulty
        }
    }

    /// Reinsert reverted transactions from reorg buffer into mempool
    ///
    /// Called after successful reorg execution to restore transactions to mempool.
    /// Revalidates transactions and maintains deterministic ordering.
    pub fn reinsert_reverted_transactions(&mut self, buffer: &crate::core::consensus::reorg::ReorgTxBuffer) -> Result<(), CoreError> {
        self.mempool.reinsert_reverted_transactions(buffer)
    }

    /// Execute a reorg by updating state and returning the reorg tx buffer
    ///
    /// This method encapsulates the Engine-level safe borrow path for reorg.
    pub fn execute_reorg_with_buffer(&mut self, reorg: &crate::core::consensus::reorg::ReorgState) -> Result<crate::core::consensus::reorg::ReorgTxBuffer, CoreError> {
        crate::core::consensus::reorg::execute_reorg_with_buffer(&self.dag, &mut self.state, reorg)
    }

    // === Internal accessors for submodules ===
    
    /// Get mutable DAG reference (used by submodules)
    pub(crate) fn dag_mut(&mut self) -> &mut Dag {
        &mut self.dag
    }

    /// Get DAG reference (used by submodules)
    pub(crate) fn dag(&self) -> &Dag {
        &self.dag
    }

    /// Get GhostDAG reference (used by submodules)
    pub(crate) fn ghostdag(&self) -> &GhostDag {
        &self.ghostdag
    }

    /// Get GhostDAG mutable reference (used by submodules)
    pub(crate) fn ghostdag_mut(&mut self) -> &mut GhostDag {
        &mut self.ghostdag
    }

    /// Get config reference (used by submodules)
    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    /// Get state mutable reference (used by submodules)
    pub(crate) fn state_mut(&mut self) -> &mut BlockchainState {
        &mut self.state
    }

    /// Get storage mutable reference (used by submodules)
    pub(crate) fn storage_mut(&mut self) -> &mut Box<dyn Storage> {
        &mut self.storage
    }

    /// Get mempool mutable reference (used by submodules)
    pub(crate) fn mempool_mut(&mut self) -> &mut mempool::Mempool {
        &mut self.mempool
    }

    /// Get genesis hash reference (used by submodules)
    pub(crate) fn genesis_hash(&self) -> &Option<Hash> {
        &self.genesis_hash
    }

    /// Set genesis hash (used by submodules)
    pub(crate) fn set_genesis_hash(&mut self, hash: Hash) {
        self.genesis_hash = Some(hash);
    }

    /// Check if genesis already set (used by submodules)
    pub(crate) fn genesis_already_set(&self) -> bool {
        self.genesis_hash.is_some()
    }

    /// Get finalized tip mutable reference (used by submodules)
    pub(crate) fn finalized_tip_mut(&mut self) -> &mut Option<Hash> {
        &mut self.finalized_tip
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creation() {
        let engine = Engine::new();
        assert_eq!(engine.get_block_count(), 0);
        assert_eq!(engine.get_genesis(), None);
    }

    #[test]
    fn test_engine_with_config() {
        let config = Config::default();
        let k = config.k;
        let engine = Engine::with_config(config);
        assert_eq!(engine.get_k(), k);
    }

    #[test]
    fn test_engine_default() {
        let engine = Engine::default();
        assert_eq!(engine.get_block_count(), 0);
    }
}
