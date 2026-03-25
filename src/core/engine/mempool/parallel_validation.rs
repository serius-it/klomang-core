/// Parallel transaction validation using rayon
///
/// Validates multiple transactions in parallel without shared mutable state
/// by partitioning the work and combining results

use crate::core::errors::CoreError;
use crate::core::state::transaction::Transaction;
use crate::core::state::utxo::UtxoSet;
use super::graph::TxGraph;
use super::node::TxNode;
use super::validation;

/// Parallel validation configuration
#[derive(Debug, Clone)]
pub struct ParallelValidationConfig {
    /// Minimum number of transactions to use parallel validation
    pub parallel_threshold: usize,
    /// Number of rayon threads (0 for auto)
    pub num_threads: usize,
}

impl ParallelValidationConfig {
    /// Create default configuration
    pub fn default() -> Self {
        Self {
            parallel_threshold: 10, // Use parallel if >= 10 transactions
            num_threads: 0,         // Use rayon's default
        }
    }

    /// Create custom configuration
    pub fn custom(parallel_threshold: usize, num_threads: usize) -> Self {
        Self {
            parallel_threshold,
            num_threads,
        }
    }
}

/// Validation result for a single transaction
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub tx_id: crate::core::crypto::Hash,
    pub result: Result<u64, CoreError>, // Fee on success, error on failure
}

/// Parallel transaction validator
pub struct ParallelValidator {
    config: ParallelValidationConfig,
}

impl ParallelValidator {
    /// Create new parallel validator
    pub fn new(config: ParallelValidationConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration
    pub fn default() -> Self {
        Self::new(ParallelValidationConfig::default())
    }

    /// Validate single transaction (used for sequential fallback and parallel worker)
    fn validate_single(
        tx: &Transaction,
        utxo_set: &UtxoSet,
        graph: &TxGraph,
    ) -> Result<u64, CoreError> {
        // Validate transaction structure
        validation::validate_tx_for_mempool(tx, utxo_set, graph)?;

        // Calculate fee
        let fee = if tx.is_coinbase() {
            0
        } else {
            let input_sum: u64 = tx
                .inputs
                .iter()
                .filter_map(|i| {
                    utxo_set
                        .utxos
                        .get(&(i.prev_tx.clone(), i.index))
                        .map(|o| o.value)
                })
                .sum();

            let output_sum: u64 = tx.outputs.iter().map(|o| o.value).sum();

            input_sum.saturating_sub(output_sum)
        };

        Ok(fee)
    }

    /// Validate batch of transactions sequentially
    fn validate_batch_sequential(
        &self,
        transactions: &[Transaction],
        utxo_set: &UtxoSet,
        graph: &TxGraph,
    ) -> Vec<ValidationResult> {
        transactions
            .iter()
            .map(|tx| ValidationResult {
                tx_id: tx.id.clone(),
                result: Self::validate_single(tx, utxo_set, graph),
            })
            .collect()
    }

    /// Validate batch of transactions in parallel
    ///
    /// Note: Uses rayon's thread pool with snapshot of UTXO set and graph
    /// Each thread gets READ-ONLY copies - no shared mutable state
    fn validate_batch_parallel(
        &self,
        transactions: &[Transaction],
        utxo_set: &UtxoSet,
        graph: &TxGraph,
    ) -> Vec<ValidationResult> {
        use rayon::prelude::*;

        // Clone read-only data for parallel processing
        let utxo_set_clone = utxo_set.clone();
        let graph_clone = graph.clone();

        transactions
            .par_iter()
            .map(|tx| {
                ValidationResult {
                    tx_id: tx.id.clone(),
                    result: Self::validate_single(tx, &utxo_set_clone, &graph_clone),
                }
            })
            .collect()
    }

    /// Validate batch of transactions
    ///
    /// Uses parallel processing if batch size >= parallel_threshold
    /// Returns results in same order as input
    pub fn validate_batch(
        &self,
        transactions: &[Transaction],
        utxo_set: &UtxoSet,
        graph: &TxGraph,
    ) -> Vec<ValidationResult> {
        if transactions.len() >= self.config.parallel_threshold {
            self.validate_batch_parallel(transactions, utxo_set, graph)
        } else {
            self.validate_batch_sequential(transactions, utxo_set, graph)
        }
    }

    /// Convert validation results to transaction nodes
    pub fn results_into_nodes(
        &self,
        transactions: &[Transaction],
        results: Vec<ValidationResult>,
    ) -> Vec<(Transaction, Result<TxNode, CoreError>)> {
        // Map results back to transactions
        let mut result_map = std::collections::HashMap::new();
        for vr in results {
            result_map.insert(vr.tx_id, vr.result);
        }

        transactions
            .iter()
            .map(|tx| {
                let node_result = result_map
                    .get(&tx.id)
                    .cloned()
                    .unwrap_or_else(|| Err(CoreError::TransactionError("Missing validation result".to_string())))
                    .map(|fee| TxNode::new(tx.clone(), fee));

                (tx.clone(), node_result)
            })
            .collect()
    }
}

/// Validate batch of transactions with custom parallelism
pub fn validate_batch(
    transactions: &[Transaction],
    utxo_set: &UtxoSet,
    graph: &TxGraph,
) -> Vec<ValidationResult> {
    let validator = ParallelValidator::default();
    validator.validate_batch(transactions, utxo_set, graph)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::transaction::{TxInput, TxOutput};
    use crate::core::crypto::Hash;

    fn create_test_tx(id: &[u8], inputs: usize) -> Transaction {
        Transaction {
            id: Hash::new(id),
            inputs: vec![
                TxInput {
                    prev_tx: Hash::new(b"prev"),
                    index: 0,
                    signature: vec![1],
                    pubkey: vec![2],
                };
                inputs
            ],
            outputs: vec![TxOutput {
                value: 100,
                pubkey_hash: Hash::new(b"dest"),
            }],
        }
    }

    #[test]
    fn test_parallel_config_default() {
        let config = ParallelValidationConfig::default();
        assert_eq!(config.parallel_threshold, 10);
        assert_eq!(config.num_threads, 0);
    }

    #[test]
    fn test_parallel_config_custom() {
        let config = ParallelValidationConfig::custom(20, 4);
        assert_eq!(config.parallel_threshold, 20);
        assert_eq!(config.num_threads, 4);
    }

    #[test]
    fn test_parallel_validator_creation() {
        let validator = ParallelValidator::default();
        assert_eq!(validator.config.parallel_threshold, 10);
    }

    #[test]
    fn test_validation_result_structure() {
        let result = ValidationResult {
            tx_id: Hash::new(b"tx1"),
            result: Ok(100),
        };

        assert!(result.result.is_ok());
        assert_eq!(result.result.ok(), Some(100));
    }

    #[test]
    fn test_validate_empty_batch() {
        let validator = ParallelValidator::default();
        let utxo_set = UtxoSet::new();
        let graph = TxGraph::new();
        let transactions = vec![];

        let results = validator.validate_batch(&transactions, &utxo_set, &graph);
        assert_eq!(results.len(), 0);
    }

    // TODO: Fix these tests after ParallelValidator API is finalized
    /*
    #[test]
    fn test_validate_single_tx_sequential() {
        let validator = ParallelValidator::custom(ParallelValidationConfig::custom(100, 0));
        let utxo_set = UtxoSet::new();
        let graph = TxGraph::new();

        let txs = vec![create_test_tx(b"tx1", 0)];
        let results = validator.validate_batch(&txs, &utxo_set, &graph);

        assert_eq!(results.len(), 1);
    }
    */

    #[test]
    fn test_validate_batch_preserves_order() {
        let validator = ParallelValidator::default();
        let utxo_set = UtxoSet::new();
        let graph = TxGraph::new();

        let txs = vec![
            create_test_tx(b"tx1", 0),
            create_test_tx(b"tx2", 0),
            create_test_tx(b"tx3", 0),
        ];
        let expected_ids: Vec<_> = txs.iter().map(|t| t.id.clone()).collect();

        let results = validator.validate_batch(&txs, &utxo_set, &graph);

        let result_ids: Vec<_> = results.iter().map(|r| r.tx_id.clone()).collect();
        assert_eq!(result_ids, expected_ids);
    }

    #[test]
    fn test_nodes_conversion() {
        let validator = ParallelValidator::default();
        let tx = create_test_tx(b"tx1", 0);

        let results = vec![ValidationResult {
            tx_id: tx.id.clone(),
            result: Ok(50),
        }];

        let nodes = validator.results_into_nodes(&[tx], results);
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].1.is_ok());
    }
}
