/// Priority mempool with DAG-aware fee market
///
/// Modular mempool architecture:
/// - node: Transaction representation with fee/rate data
/// - graph: Transaction DAG management
/// - validation: UTXO, signature, double-spend checks
/// - selection: Fee-rate based transaction selection
/// - package: Transaction package (tx + all ancestors) for accurate fee calculation
/// - rbf: Replace-by-fee support for fee bumping
/// - eviction: Mempool size management with fairness
/// - buckets: Priority bucket management (High/Medium/Low)
/// - parallel_validation: Rayon-based parallel transaction validation
/// - fairness: Age bonus and deterministic ordering

pub mod node;
pub mod graph;
pub mod validation;
pub mod selection;
pub mod package;
pub mod rbf;
pub mod eviction;
pub mod buckets;
pub mod parallel_validation;
pub mod fairness;

pub use node::TxNode;
pub use graph::TxGraph;
pub use selection::TransactionSelector;
pub use package::{Package, build_package, select_packages};
pub use rbf::{RbfConfig, execute_rbf};
pub use eviction::{EvictionPolicy, MempoolStats, apply_eviction_if_needed, calculate_mempool_size};
pub use buckets::{BucketedMempool, BucketConfig, PriorityLevel, BucketStats};
pub use parallel_validation::{ParallelValidator, ParallelValidationConfig, validate_batch};
pub use fairness::{FairnessConfig, FairnessScore, order_by_fairness};

use crate::core::crypto::Hash;
use crate::core::state::transaction::Transaction;
use crate::core::errors::CoreError;
use crate::core::state::utxo::UtxoSet;

/// Main mempool coordinator
///
/// Manages transaction pool with:
/// - Priority-based selection (fee-rate)
/// - DAG-aware dependencies
/// - UTXO and signature validation
/// - Block building integration
/// - Priority buckets (High/Medium/Low)
/// - Eviction policy with size limits
/// - Parallel validation using rayon
/// - Fairness ordering with age bonus
#[derive(Debug)]
pub struct Mempool {
    /// Transaction DAG
    graph: TxGraph,
    /// Transaction selection engine
    selector: TransactionSelector,
    /// UTXO set for validation
    utxo_set: UtxoSet,
    /// Maximum block size (bytes)
    max_block_size: u64,
    /// Priority buckets for transactions
    buckets: BucketedMempool,
    /// Eviction policy
    eviction_policy: EvictionPolicy,
    /// Parallel validation configuration
    validator_config: ParallelValidationConfig,
    /// Fairness ordering configuration
    fairness_config: FairnessConfig,
}

impl Mempool {
    /// Create new mempool with default configuration
    pub fn new() -> Self {
        Self::with_config(1_000_000) // 1MB default block size
    }

    /// Create mempool with custom block size
    pub fn with_config(max_block_size: u64) -> Self {
        Self {
            graph: TxGraph::new(),
            selector: TransactionSelector::new(max_block_size),
            utxo_set: UtxoSet::new(),
            max_block_size,
            buckets: BucketedMempool::default(),
            eviction_policy: EvictionPolicy::default(),
            validator_config: ParallelValidationConfig::default(),
            fairness_config: FairnessConfig::default(),
        }
    }

    /// Submit a transaction to the mempool
    ///
    /// Validates:
    /// - No duplicates
    /// - UTXO existence and sufficiency
    /// - Schnorr signatures (structure validation)
    /// - No double-spending
    /// - Transaction dependencies in mempool
    /// - RBF (Replace-by-Fee) if transaction conflicts with existing
    /// - Applies eviction if mempool exceeds size limit
    pub fn submit_tx(&mut self, tx: Transaction) -> Result<(), CoreError> {
        // Check for duplicate
        if self.graph.get_tx(&tx.id).is_some() {
            return Err(CoreError::TransactionError(
                "Duplicate transaction".to_string(),
            ));
        }

        // Validate transaction
        validation::validate_tx_for_mempool(&tx, &self.utxo_set, &self.graph)?;

        // Calculate fee (output diff for valid txs, 0 for coinbase)
        let fee = if tx.is_coinbase() {
            0
        } else {
            let input_sum: u64 = tx
                .inputs
                .iter()
                .filter_map(|i| {
                    self.utxo_set
                        .utxos
                        .get(&(i.prev_tx.clone(), i.index))
                        .map(|o| o.value)
                })
                .sum();

            let output_sum: u64 = tx.outputs.iter().map(|o| o.value).sum();

            input_sum.saturating_sub(output_sum)
        };

        // Try RBF replacement if transaction conflicts
        let rbf_config = RbfConfig::default();
        if let Some(_removed) = execute_rbf(&tx, fee, &mut self.graph, &rbf_config)? {
            // RBF successful - transaction replaced conflicting one
            // Note: _removed contains descendants that were evicted and could be resubmitted
            return Ok(());
        }

        // No RBF needed - add normally
        let node = TxNode::new(tx, fee);
        self.graph.add_tx(node.clone())?;

        // Add to priority buckets
        self.buckets.add_tx(node)?;

        // Apply eviction if needed (respects the dependency constraint)
        let _evicted = eviction::apply_eviction_if_needed(&mut self.graph, &self.eviction_policy)?;

        Ok(())
    }

    /// Remove confirmed transactions after block creation
    /// Also removes from priority buckets
    pub fn remove_confirmed(&mut self, tx_ids: &[Hash]) -> Result<(), CoreError> {
        for tx_id in tx_ids {
            self.graph.remove_tx(tx_id)?;
            // Also remove from priority buckets
            let _ = self.buckets.remove_tx(tx_id);
        }
        Ok(())
    }

    /// Select transactions for block with fee-rate priority
    ///
    /// Returns transactions ordered by:
    /// 1. Fee rate (satoshis per byte)
    /// 2. Transaction dependencies (topological order)
    /// 3. Block size limit
    pub fn select_txs_for_block(&self) -> Result<Vec<Transaction>, CoreError> {
        self.selector.select_transactions_ordered(&self.graph)
    }

    /// Select transactions with maximum count
    pub fn select_txs_limited(&self, max_count: usize) -> Result<Vec<Transaction>, CoreError> {
        let mut txs = self.select_txs_for_block()?;
        txs.truncate(max_count);
        Ok(txs)
    }

    /// Select transactions with fairness ordering applied
    ///
    /// Uses:
    /// 1. Priority buckets (High → Medium → Low)
    /// 2. Fairness scoring (fee_rate + age bonus)
    /// 3. Topological ordering (parents first)
    pub fn select_txs_with_fairness(&self) -> Result<Vec<Transaction>, CoreError> {
        let all_nodes = self.graph.get_ready_txs()?;

        if all_nodes.is_empty() {
            return Ok(Vec::new());
        }

        // Apply fairness ordering
        let ordered = fairness::order_by_fairness(all_nodes, &self.fairness_config)?;

        // Respect block size
        let mut selected = Vec::new();
        let mut total_size = 0u64;

        for node in ordered {
            let tx_size = node.size_bytes();
            if total_size + tx_size > self.max_block_size {
                break;
            }
            selected.push(node.tx.clone());
            total_size += tx_size;
        }

        Ok(selected)
    }

    /// Select transactions from specific priority bucket
    pub fn select_txs_from_bucket(&self, priority: buckets::PriorityLevel) -> Result<Vec<Transaction>, CoreError> {
        let bucket_nodes = self.buckets.get_bucket(priority);

        if bucket_nodes.is_empty() {
            return Ok(Vec::new());
        }

        let mut selected = Vec::new();
        let mut total_size = 0u64;

        for node in bucket_nodes {
            let tx_size = node.size_bytes();
            if total_size + tx_size > self.max_block_size {
                break;
            }
            selected.push(node.tx.clone());
            total_size += tx_size;
        }

        Ok(selected)
    }

    /// Validate batch of transactions in parallel
    pub fn validate_batch_parallel(
        &self,
        transactions: &[Transaction],
    ) -> Vec<parallel_validation::ValidationResult> {
        let validator = parallel_validation::ParallelValidator::new(
            self.validator_config.clone()
        );
        validator.validate_batch(transactions, &self.utxo_set, &self.graph)
    }

    /// Get bucket statistics for monitoring
    pub fn get_bucket_stats(&self) -> buckets::BucketStats {
        self.buckets.get_stats()
    }

    /// Get mempool size statistics for eviction monitoring
    pub fn get_size_stats(&self) -> Result<eviction::MempoolStats, CoreError> {
        eviction::get_stats(&self.graph, &self.eviction_policy)
    }

    /// Check if mempool is at or near capacity
    pub fn is_near_capacity(&self) -> Result<bool, CoreError> {
        let stats = self.get_size_stats()?;
        Ok(stats.should_warn(&self.eviction_policy))
    }

    /// Get a transaction by ID
    pub fn get_tx(&self, tx_id: &Hash) -> Option<&TxNode> {
        self.graph.get_tx(tx_id)
    }

    /// Get transaction count in mempool
    pub fn tx_count(&self) -> usize {
        self.graph.tx_count()
    }

    /// Get valid transaction count
    pub fn valid_tx_count(&self) -> usize {
        self.graph.get_valid_txs().len()
    }

    /// Mark transaction as invalid (and its children)
    pub fn invalidate_tx(&mut self, tx_id: &Hash) -> Result<(), CoreError> {
        self.graph.set_valid(tx_id, false)
    }

    /// Update UTXO set after state change
    pub fn update_utxo_set(&mut self, utxo_set: UtxoSet) {
        self.utxo_set = utxo_set;
    }

    /// Get current UTXO set (for external validation)
    pub fn get_utxo_set(&self) -> &UtxoSet {
        &self.utxo_set
    }

    /// Check for cycles in transaction DAG (should never happen)
    pub fn has_cycles(&self) -> Result<bool, CoreError> {
        self.graph.has_cycles()
    }

    /// Reinsert reverted transactions from reorg buffer
    ///
    /// Revalidates each transaction against current state:
    /// - Skip invalid/double-spent transactions
    /// - Reattach dependency graph
    /// - Maintain deterministic ordering
    pub fn reinsert_reverted_transactions(&mut self, buffer: &crate::core::consensus::reorg::ReorgTxBuffer) -> Result<(), CoreError> {
        if buffer.is_empty() {
            return Ok(());
        }

        // Sort transactions deterministically by ID for consistent reinsertion
        let mut sorted_txs = buffer.reverted.clone();
        sorted_txs.sort_by(|a, b| a.id.cmp(&b.id));

        for tx in sorted_txs {
            // Revalidate transaction against current state
            match validation::validate_tx_for_mempool(&tx, &self.utxo_set, &self.graph) {
                Ok(_) => {
                    // Transaction is still valid - reinsert
                    let fee = if tx.is_coinbase() {
                        0
                    } else {
                        let input_sum: u64 = tx
                            .inputs
                            .iter()
                            .filter_map(|i| {
                                self.utxo_set
                                    .utxos
                                    .get(&(i.prev_tx.clone(), i.index))
                                    .map(|o| o.value)
                            })
                            .sum();

                        let output_sum: u64 = tx.outputs.iter().map(|o| o.value).sum();

                        input_sum.saturating_sub(output_sum)
                    };

                    let node = TxNode::new(tx, fee);
                    self.graph.add_tx(node.clone())?;
                    self.buckets.add_tx(node)?;
                }
                Err(_) => {
                    // Transaction is invalid/double-spent - skip silently
                    // This is expected during reorg as some txs may become invalid
                }
            }
        }

        // Apply eviction if needed after reinsertion
        let _evicted = eviction::apply_eviction_if_needed(&mut self.graph, &self.eviction_policy)?;

        Ok(())
    }

    /// Get high-level statistics
    pub fn get_stats(&self) -> Result<MempoolStats, CoreError> {
        eviction::get_stats(&self.graph, &self.eviction_policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::transaction::{TxInput, TxOutput};

    fn create_test_tx(id: &[u8]) -> Transaction {
        Transaction {
            id: Hash::new(id),
            inputs: vec![],
            outputs: vec![TxOutput {
                value: 100,
                pubkey_hash: Hash::new(b"dest"),
            }],
        }
    }

    #[test]
    fn test_mempool_creation() {
        let mempool = Mempool::new();
        assert_eq!(mempool.tx_count(), 0);
    }

    #[test]
    fn test_mempool_submit_coinbase() {
        let mut mempool = Mempool::new();
        let tx = create_test_tx(b"cb1");

        assert!(mempool.submit_tx(tx).is_ok());
        assert_eq!(mempool.tx_count(), 1);
    }

    #[test]
    fn test_mempool_duplicate_rejection() {
        let mut mempool = Mempool::new();
        let tx = create_test_tx(b"tx1");

        mempool.submit_tx(tx.clone()).ok();

        let result = mempool.submit_tx(tx);
        assert!(result.is_err());
    }

    #[test]
    fn test_mempool_stats() {
        let mut mempool = Mempool::new();
        let tx = create_test_tx(b"tx1");

        mempool.submit_tx(tx).ok();

        let stats = mempool.get_stats().ok();
        assert_eq!(stats.map(|s| s.tx_count), Some(1));
    }

    #[test]
    fn test_mempool_select_transactions() {
        let mut mempool = Mempool::new();
        let tx = create_test_tx(b"tx1");

        mempool.submit_tx(tx).ok();

        let selected = mempool.select_txs_for_block();
        assert!(selected.is_ok());
        assert_eq!(selected.ok().map(|v| v.len()), Some(1));
    }

    #[test]
    fn test_mempool_remove_confirmed() {
        let mut mempool = Mempool::new();
        let tx = create_test_tx(b"tx1");
        let tx_id = tx.id.clone();

        mempool.submit_tx(tx).ok();
        assert_eq!(mempool.tx_count(), 1);

        mempool.remove_confirmed(&[tx_id]).ok();
        assert_eq!(mempool.tx_count(), 0);
    }

    #[test]
    fn test_mempool_invalidate_tx() {
        let mut mempool = Mempool::new();
        let tx = create_test_tx(b"tx1");
        let tx_id = tx.id.clone();

        mempool.submit_tx(tx).ok();
        assert_eq!(mempool.valid_tx_count(), 1);

        mempool.invalidate_tx(&tx_id).ok();
        assert_eq!(mempool.valid_tx_count(), 0);
    }

    #[test]
    fn test_reinsert_reverted_transactions_empty_buffer() {
        let mut mempool = Mempool::new();
        let buffer = crate::core::consensus::reorg::ReorgTxBuffer::new();

        assert!(mempool.reinsert_reverted_transactions(&buffer).is_ok());
        assert_eq!(mempool.tx_count(), 0);
    }

    #[test]
    fn test_reinsert_reverted_transactions_valid_tx() {
        let mut mempool = Mempool::new();
        let tx = create_test_tx(b"revert_tx");

        let buffer = crate::core::consensus::reorg::ReorgTxBuffer::with_transactions(vec![tx.clone()]);

        assert!(mempool.reinsert_reverted_transactions(&buffer).is_ok());
        assert_eq!(mempool.tx_count(), 1);
        assert_eq!(mempool.get_tx(&tx.id).unwrap().fee_rate, 0);
    }

    #[test]
    fn test_reinsert_reverted_transactions_invalid_tx() {
        let mut mempool = Mempool::new();

        // Create a transaction that spends from non-existent UTXO
        let invalid_tx = Transaction {
            id: Hash::new(b"invalid"),
            inputs: vec![TxInput {
                prev_tx: Hash::new(b"nonexistent"),
                index: 0,
                signature: vec![],
                pubkey: vec![],
            }],
            outputs: vec![TxOutput {
                value: 50,
                pubkey_hash: Hash::new(b"dest"),
            }],
        };

        let buffer = crate::core::consensus::reorg::ReorgTxBuffer::with_transactions(vec![invalid_tx]);

        // Should not fail, but should skip invalid transaction
        assert!(mempool.reinsert_reverted_transactions(&buffer).is_ok());
        assert_eq!(mempool.tx_count(), 0);
    }

    #[test]
    fn test_reinsert_reverted_transactions_deterministic_ordering() {
        let mut mempool = Mempool::new();

        // Create transactions with IDs that would sort differently
        let tx1 = Transaction {
            id: Hash::new(b"z_tx"),
            inputs: vec![],
            outputs: vec![TxOutput {
                value: 100,
                pubkey_hash: Hash::new(b"dest1"),
            }],
        };
        let tx2 = Transaction {
            id: Hash::new(b"a_tx"),
            inputs: vec![],
            outputs: vec![TxOutput {
                value: 200,
                pubkey_hash: Hash::new(b"dest2"),
            }],
        };

        let buffer = crate::core::consensus::reorg::ReorgTxBuffer::with_transactions(vec![tx1.clone(), tx2.clone()]);

        assert!(mempool.reinsert_reverted_transactions(&buffer).is_ok());
        assert_eq!(mempool.tx_count(), 2);

        // Verify both transactions are present
        assert!(mempool.get_tx(&tx1.id).is_some());
        assert!(mempool.get_tx(&tx2.id).is_some());
    }
}
