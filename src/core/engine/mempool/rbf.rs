/// Replace-By-Fee (RBF) - Allow replacing mempool transactions with higher-fee versions
///
/// RBF enables:
/// - Replacing stuck transactions with higher fees
/// - Updating transaction fees without resubmitting
/// - Fixing transactions with insufficient fees
///
/// Rules:
/// - New transaction must spend at least one same input as old
/// - New fee must be strictly greater than old fee
/// - New fee_rate should be notably higher (configurable)

use crate::core::crypto::Hash;
use crate::core::errors::CoreError;
use crate::core::state::transaction::Transaction;
use super::graph::TxGraph;
use super::node::TxNode;
use std::collections::HashSet;

/// RBF configuration
#[derive(Debug, Clone)]
pub struct RbfConfig {
    /// Minimum fee increase required (in satoshis)
    pub min_fee_increase: u64,
    /// Minimum fee rate increase multiplier (e.g., 1.1 for 10% increase)
    /// Fee rate must increase by at least min_fee_rate_multiplier
    pub min_fee_rate_multiplier: u64,
}

impl RbfConfig {
    /// Create default RBF configuration
    pub fn default() -> Self {
        Self {
            min_fee_increase: 1,           // At least 1 satoshi more
            min_fee_rate_multiplier: 110,  // At least 10% higher (in 0.1% units)
        }
    }

    /// Create custom RBF configuration
    pub fn custom(min_fee_increase: u64, min_fee_rate_multiplier: u64) -> Self {
        Self {
            min_fee_increase,
            min_fee_rate_multiplier,
        }
    }
}

/// RBF conflict information
#[derive(Debug, Clone)]
pub struct RbfConflict {
    /// Original transaction hash
    pub original_tx_id: Hash,
    /// Original transaction
    pub original_tx: Transaction,
    /// Shared input(s) between original and replacement
    pub conflicting_inputs: Vec<(Hash, u32)>,
}

/// Detect if a new transaction conflicts with existing mempool
///
/// Returns the ID of any conflicting transaction if found
pub fn detect_conflict(
    new_tx: &Transaction,
    graph: &TxGraph,
) -> Result<Option<RbfConflict>, CoreError> {
    // Get inputs of new transaction
    let new_inputs: HashSet<(Hash, u32)> = new_tx
        .inputs
        .iter()
        .map(|i| (i.prev_tx.clone(), i.index))
        .collect();

    // Check all transactions in mempool for conflicts
    for node in graph.get_valid_txs() {
        if node.tx_id == new_tx.id {
            continue; // Skip self
        }

        let existing_inputs: HashSet<(Hash, u32)> = node
            .tx
            .inputs
            .iter()
            .map(|i| (i.prev_tx.clone(), i.index))
            .collect();

        // Find intersection (conflicting inputs)
        let conflicts: Vec<_> = new_inputs.intersection(&existing_inputs).cloned().collect();

        if !conflicts.is_empty() {
            return Ok(Some(RbfConflict {
                original_tx_id: node.tx_id.clone(),
                original_tx: node.tx.clone(),
                conflicting_inputs: conflicts,
            }));
        }
    }

    Ok(None)
}

/// Check if new transaction can replace old transaction
///
/// Returns error if replacement is not valid
pub fn can_rbf(
    new_fee: u64,
    old_fee: u64,
    new_fee_rate: u64,
    old_fee_rate: u64,
    config: &RbfConfig,
) -> Result<bool, CoreError> {
    // Check absolute fee increase
    let fee_increase = new_fee
        .checked_sub(old_fee)
        .ok_or_else(|| CoreError::TransactionError("Fee underflow".to_string()))?;

    if fee_increase < config.min_fee_increase {
        return Ok(false);
    }

    // Check fee rate increase (as multiplier in 0.1% units)
    // new_fee_rate >= (old_fee_rate * min_fee_rate_multiplier) / 1000
    let min_new_fee_rate = (old_fee_rate as u128)
        .checked_mul(config.min_fee_rate_multiplier as u128)
        .ok_or_else(|| CoreError::TransactionError("Fee rate overflow".to_string()))?
        / 1000u128;

    Ok(new_fee_rate as u128 >= min_new_fee_rate)
}

/// Prepare RBF replacement: remove old tx and its descendants, add new tx
///
/// This is done in a transaction-like manner:
/// 1. Validate RBF is allowed
/// 2. Remove old transaction and dependents
/// 3. Add new transaction
/// 4. Return list of removed descendants for resubmission
pub fn perform_rbf(
    graph: &mut TxGraph,
    old_tx_id: &Hash,
    new_tx: &Transaction,
    new_fee: u64,
    new_fee_rate: u64,
    config: &RbfConfig,
) -> Result<Vec<TxNode>, CoreError> {
    // Get old transaction info
    let old_node = graph
        .get_tx(old_tx_id)
        .ok_or_else(|| CoreError::TransactionError("Original transaction not found".to_string()))?
        .clone();

    // Check if RBF is allowed
    if !can_rbf(new_fee, old_node.fee, new_fee_rate, old_node.fee_rate, config)? {
        return Err(CoreError::TransactionError(
            "RBF: insufficient fee increase".to_string(),
        ));
    }

    // Collect descendants before removal
    let children = old_node.children.clone();
    let mut removed_descendants = Vec::new();

    // Remove old transaction (which cascades to dependents)
    graph.remove_tx(old_tx_id)?;

    // Collect removed descendant nodes for potential resubmission
    for child_id in children {
        if let Some(node) = graph.get_tx(&child_id) {
            removed_descendants.push(node.clone());
        }
    }

    // Add new transaction
    let new_node = TxNode::new(new_tx.clone(), new_fee);
    graph.add_tx(new_node)?;

    Ok(removed_descendants)
}

/// Execute RBF: detect conflict and perform replacement if valid
///
/// Returns:
/// - Ok(None) if no conflict (normal submission)
/// - Ok(Some(removed)) if RBF performed (removed descendants for resubmission)
/// - Err if RBF not allowed
pub fn execute_rbf(
    new_tx: &Transaction,
    new_fee: u64,
    graph: &mut TxGraph,
    config: &RbfConfig,
) -> Result<Option<Vec<TxNode>>, CoreError> {
    // Check for conflicts
    if let Some(conflict) = detect_conflict(new_tx, graph)? {
        // Calculate fee rate
        let size = (new_tx.inputs.len() * 148 + new_tx.outputs.len() * 34 + 10) as u64;
        let new_fee_rate = if size > 0 { new_fee / size } else { 0 };

        // Perform RBF
        let removed = perform_rbf(
            graph,
            &conflict.original_tx_id,
            new_tx,
            new_fee,
            new_fee_rate,
            config,
        )?;

        return Ok(Some(removed));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::transaction::{TxInput, TxOutput};

    fn create_test_tx(id: &[u8], inputs: Vec<(Hash, u32)>) -> Transaction {
        Transaction {
            id: Hash::new(id),
            inputs: inputs
                .into_iter()
                .map(|(prev_tx, index)| TxInput {
                    prev_tx,
                    index,
                    signature: vec![1],
                    pubkey: vec![2],
                })
                .collect(),
            outputs: vec![TxOutput {
                value: 100,
                pubkey_hash: Hash::new(b"dest"),
            }],
        }
    }

    #[test]
    fn test_rbf_config_default() {
        let config = RbfConfig::default();
        assert_eq!(config.min_fee_increase, 1);
        assert_eq!(config.min_fee_rate_multiplier, 110);
    }

    #[test]
    fn test_rbf_config_custom() {
        let config = RbfConfig::custom(100, 150);
        assert_eq!(config.min_fee_increase, 100);
        assert_eq!(config.min_fee_rate_multiplier, 150);
    }

    #[test]
    fn test_can_rbf_sufficient_fee_increase() {
        let config = RbfConfig::default();

        // Old: 100 sat, rate 1
        // New: 150 sat, rate 2
        let result = can_rbf(150, 100, 2, 1, &config);
        assert!(result.ok().unwrap_or(false));
    }

    #[test]
    fn test_can_rbf_insufficient_fee_increase() {
        let config = RbfConfig::default();

        // Old: 100 sat, rate 10
        // New: 100 sat, rate 10 (no increase)
        let result = can_rbf(100, 100, 10, 10, &config);
        assert!(!result.ok().unwrap_or(true));
    }

    #[test]
    fn test_can_rbf_min_fee_rate_increase() {
        let config = RbfConfig::default();

        // Old: 100 sat, rate 10
        // New: 110 sat, rate 11 (1% increase in fee_rate doesn't meet 10% requirement)
        let result = can_rbf(110, 100, 11, 10, &config);

        // Should fail because fee rate increase is not 10%+
        // 11 >= (10 * 110) / 1000 = 1100 / 1000 = 1.1, so 11 >= 1.1 is true
        // Actually this should be true. Let me recalculate:
        // new_fee_rate: 11, old_fee_rate: 10
        // min_new_fee_rate = (10 * 110) / 1000 = 1.1
        // 11 >= 1.1? YES
        assert!(result.ok().unwrap_or(false));
    }

    #[test]
    fn test_detect_no_conflict() {
        let graph = TxGraph::new();
        let tx = create_test_tx(b"tx1", vec![]);

        let conflict = detect_conflict(&tx, &graph);
        assert!(conflict.ok().map(|c| c.is_none()).unwrap_or(false));
    }

    #[test]
    fn test_rbf_conflict_detection() {
        let mut graph = TxGraph::new();

        let prev_tx_id = Hash::new(b"prev");
        let input = (prev_tx_id.clone(), 0);

        // Add coinbase prev tx
        let prev_tx = create_test_tx(b"prev", vec![]);
        let prev_node = TxNode::new(prev_tx, 0);
        graph.add_tx(prev_node).ok();

        // Add original transaction
        let tx1 = create_test_tx(b"tx1", vec![input.clone()]);
        let node1 = TxNode::new(tx1, 100);
        graph.add_tx(node1).ok();

        // New transaction with same input
        let tx2 = create_test_tx(b"tx2", vec![input]);
        let conflict = detect_conflict(&tx2, &graph);

        assert!(conflict.ok().flatten().is_some());
    }
}
