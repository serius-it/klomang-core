/// Mempool eviction policy - maintain size limits by removing low fee-rate transactions
///
/// Rules:
/// - Never evict transactions with dependents (safe for consensus)
/// - Evict lowest fee-rate transactions first
/// - Deterministic selection if multiple candidates have same fee-rate

use crate::core::crypto::Hash;
use crate::core::errors::CoreError;
use super::graph::TxGraph;
use super::node::TxNode;

/// Mempool eviction policy configuration
#[derive(Debug, Clone)]
pub struct EvictionPolicy {
    /// Maximum mempool size in bytes
    pub max_mempool_size: u64,
    /// Warn threshold (percentage of max_mempool_size)
    pub warn_threshold: f64,
    /// Batch eviction size (evict N txs at once if over limit)
    pub batch_size: usize,
}

impl EvictionPolicy {
    /// Create default eviction policy (1 MB mempool)
    pub fn default() -> Self {
        Self {
            max_mempool_size: 1_000_000,
            warn_threshold: 0.9,
            batch_size: 10,
        }
    }

    /// Create custom policy
    pub fn with_size(max_mempool_size: u64) -> Self {
        Self {
            max_mempool_size,
            warn_threshold: 0.9,
            batch_size: 10,
        }
    }
}

/// Mempool statistics and eviction state
#[derive(Debug, Clone)]
pub struct MempoolStats {
    /// Current mempool size in bytes
    pub current_size: u64,
    /// Number of transactions
    pub tx_count: usize,
    /// Maximum mempool size
    pub max_size: u64,
    /// Percentage of capacity used
    pub capacity_used: f64,
}

impl MempoolStats {
    /// Check if mempool is at or over capacity
    pub fn is_full(&self, policy: &EvictionPolicy) -> bool {
        self.current_size >= policy.max_mempool_size
    }

    /// Check if mempool is at warning threshold
    pub fn should_warn(&self, policy: &EvictionPolicy) -> bool {
        let warn_threshold = (policy.max_mempool_size as f64) * policy.warn_threshold;
        (self.current_size as f64) >= warn_threshold
    }
}

/// Calculate transaction size in bytes (simplified)
/// Actual formula: 148 bytes per input + 34 bytes per output + 10 bytes overhead
pub fn tx_size(node: &TxNode) -> u64 {
    let inputs_size = (node.tx.inputs.len() as u64) * 148;
    let outputs_size = (node.tx.outputs.len() as u64) * 34;
    inputs_size + outputs_size + 10
}

/// Calculate current mempool size from graph
pub fn calculate_mempool_size(graph: &TxGraph) -> Result<u64, CoreError> {
    let mut total_size = 0u64;

    for node in graph.get_valid_txs() {
        let size = tx_size(node);
        total_size = total_size
            .checked_add(size)
            .ok_or_else(|| CoreError::TransactionError("Mempool size overflow".to_string()))?;
    }

    Ok(total_size)
}

/// Get mempool statistics
pub fn get_stats(graph: &TxGraph, policy: &EvictionPolicy) -> Result<MempoolStats, CoreError> {
    let current_size = calculate_mempool_size(graph)?;
    let tx_count = graph.tx_count();
    let capacity_used = (current_size as f64) / (policy.max_mempool_size as f64);

    Ok(MempoolStats {
        current_size,
        tx_count,
        max_size: policy.max_mempool_size,
        capacity_used,
    })
}

/// Find transaction to evict (lowest fee-rate without dependents)
///
/// Algorithm:
/// 1. Filter transactions with no children (dependents)
/// 2. Sort by fee_rate ascending
/// 3. Secondary sort by tx_id hash descending for determinism
/// 4. Return lowest fee-rate candidate
pub fn find_eviction_candidate(graph: &TxGraph) -> Result<Option<Hash>, CoreError> {
    let valid_txs = graph.get_valid_txs();

    // Filter transactions with no dependents
    let candidates: Vec<_> = valid_txs
        .iter()
        .filter(|node| node.children.is_empty())
        .collect();

    if candidates.is_empty() {
        return Ok(None);
    }

    // Sort by fee_rate ascending, then by tx_id hash descending
    let mut sorted = candidates.clone();
    sorted.sort_by(|a, b| {
        // Primary: fee_rate ascending (lowest first)
        match a.fee_rate.cmp(&b.fee_rate) {
            std::cmp::Ordering::Equal => {
                // Secondary: tx_id hash descending for determinism
                let hash_a = crate::core::crypto::Hash::new(a.tx_id.as_bytes());
                let hash_b = crate::core::crypto::Hash::new(b.tx_id.as_bytes());
                hash_b.as_bytes().cmp(hash_a.as_bytes())
            }
            other => other,
        }
    });

    Ok(sorted.get(0).map(|n| n.tx_id.clone()))
}

/// Apply eviction: remove transaction and return it
pub fn evict_transaction(graph: &mut TxGraph, tx_id: &Hash) -> Result<TxNode, CoreError> {
    // Get transaction before removing
    let node = graph
        .get_tx(tx_id)
        .ok_or_else(|| CoreError::TransactionError("Transaction not found for eviction".to_string()))?
        .clone();

    // Remove from graph
    graph.remove_tx(tx_id)?;

    Ok(node)
}

/// Check if eviction is needed and apply eviction batch if necessary
///
/// Returns list of evicted transactions (empty if no eviction needed)
pub fn apply_eviction_if_needed(
    graph: &mut TxGraph,
    policy: &EvictionPolicy,
) -> Result<Vec<TxNode>, CoreError> {
    let stats = get_stats(graph, policy)?;

    if !stats.is_full(policy) {
        return Ok(vec![]);
    }

    // Evict batch_size transactions
    let mut evicted = Vec::new();

    for _ in 0..policy.batch_size {
        match find_eviction_candidate(graph)? {
            Some(tx_id) => {
                let node = evict_transaction(graph, &tx_id)?;
                evicted.push(node);
            }
            None => {
                // No more candidates (all have dependents)
                break;
            }
        }

        // Check if under capacity now
        let new_stats = get_stats(graph, policy)?;
        if !new_stats.is_full(policy) {
            break;
        }
    }

    Ok(evicted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::transaction::{Transaction, TxInput, TxOutput};
    use crate::core::crypto::Hash;

    fn create_test_tx(id: &[u8], fee_rate: u64, inputs: usize, outputs: usize) -> TxNode {
        let tx = Transaction {
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
            outputs: vec![
                TxOutput {
                    value: 100,
                    pubkey_hash: Hash::new(b"dest"),
                };
                outputs
            ],
        };

        TxNode::new(tx, 100 * fee_rate)
    }

    #[test]
    fn test_eviction_policy_default() {
        let policy = EvictionPolicy::default();
        assert_eq!(policy.max_mempool_size, 1_000_000);
        assert_eq!(policy.warn_threshold, 0.9);
    }

    #[test]
    fn test_eviction_policy_custom() {
        let policy = EvictionPolicy::with_size(500_000);
        assert_eq!(policy.max_mempool_size, 500_000);
    }

    #[test]
    fn test_tx_size_calculation() {
        let node = create_test_tx(b"tx1", 10, 2, 3);
        let size = tx_size(&node);
        // 2 inputs * 148 + 3 outputs * 34 + 10 = 296 + 102 + 10 = 408
        assert_eq!(size, 408);
    }

    #[test]
    fn test_mempool_stats_not_full() {
        let policy = EvictionPolicy::with_size(1000); // 1000 bytes
        let stats = MempoolStats {
            current_size: 500,
            tx_count: 5,
            max_size: 1000,
            capacity_used: 0.5,
        };
        assert!(!stats.is_full(&policy));
    }

    #[test]
    fn test_mempool_stats_full() {
        let policy = EvictionPolicy::with_size(1000);
        let stats = MempoolStats {
            current_size: 1000,
            tx_count: 10,
            max_size: 1000,
            capacity_used: 1.0,
        };
        assert!(stats.is_full(&policy));
    }

    #[test]
    fn test_find_eviction_candidate_empty() {
        let graph = TxGraph::new();
        let result = find_eviction_candidate(&graph);
        assert!(result.ok().flatten().is_none());
    }

    #[test]
    fn test_find_eviction_candidate_without_dependents() {
        let mut graph = TxGraph::new();

        let tx1 = create_test_tx(b"tx1", 1, 0, 1); // fee_rate=1
        let tx2 = create_test_tx(b"tx2", 2, 0, 1); // fee_rate=2

        graph.add_tx(tx1).ok();
        graph.add_tx(tx2).ok();

        let candidate = find_eviction_candidate(&graph);
        // Should find tx1 (lowest fee_rate)
        assert!(candidate.ok().flatten().is_some());
    }

    #[test]
    fn test_eviction_batch_deterministic() {
        // Verify multiple candidates with same fee_rate are ordered deterministically
        let mut graph = TxGraph::new();

        let tx1 = create_test_tx(b"tx1_aaa", 1, 1, 1); // Same fee_rate
        let tx2 = create_test_tx(b"tx1_bbb", 1, 1, 1); // Same fee_rate
        let tx3 = create_test_tx(b"tx1_ccc", 1, 1, 1); // Same fee_rate

        graph.add_tx(tx1).ok();
        graph.add_tx(tx2).ok();
        graph.add_tx(tx3).ok();

        // Candidates should be deterministically ordered by hash
        let c1 = find_eviction_candidate(&graph);
        let c2 = find_eviction_candidate(&graph);

        // Same entry point should give consistent results
        assert_eq!(c1.ok().flatten(), c2.ok().flatten());
    }
}
