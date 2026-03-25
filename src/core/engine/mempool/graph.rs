/// Transaction fee-based DAG for the mempool
///
/// Manages transaction relationships, dependencies, and status
/// Enables DAG-aware transaction selection

use crate::core::crypto::Hash;
use crate::core::errors::CoreError;
use super::node::TxNode;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct TxGraph {
    /// All transactions indexed by hash
    nodes: HashMap<Hash, TxNode>,
    /// Transaction validation status
    valid_txs: HashMap<Hash, bool>,
}

impl TxGraph {
    /// Create a new empty transaction graph
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            valid_txs: HashMap::new(),
        }
    }

    /// Add a transaction to the graph
    pub fn add_tx(&mut self, node: TxNode) -> Result<(), CoreError> {
        let tx_id = node.tx_id.clone();

        if self.nodes.contains_key(&tx_id) {
            return Err(CoreError::TransactionError(
                "Transaction already in mempool".to_string(),
            ));
        }

        // Check if parents exist (unless coinbase)
        if !node.is_coinbase() {
            for parent_id in &node.parents {
                if !self.nodes.contains_key(parent_id) {
                    return Err(CoreError::TransactionError(format!(
                        "Parent transaction {} not found in mempool",
                        parent_id
                    )));
                }
            }
        }

        self.nodes.insert(tx_id.clone(), node);
        self.valid_txs.insert(tx_id, true);

        Ok(())
    }

    /// Remove a transaction and its children
    pub fn remove_tx(&mut self, tx_id: &Hash) -> Result<(), CoreError> {
        if !self.nodes.contains_key(tx_id) {
            return Err(CoreError::TransactionError("Transaction not found".to_string()));
        }

        // Collect children to remove recursively
        let children: Vec<Hash> = self
            .nodes
            .get(tx_id)
            .map(|n| n.children.clone().into_iter().collect())
            .unwrap_or_default();

        // Remove the transaction
        self.nodes.remove(tx_id);
        self.valid_txs.remove(tx_id);

        // Recursively remove children
        for child_id in children {
            let _ = self.remove_tx(&child_id);
        }

        Ok(())
    }

    /// Get a transaction node by ID
    pub fn get_tx(&self, tx_id: &Hash) -> Option<&TxNode> {
        self.nodes.get(tx_id)
    }

    /// Mark a transaction as valid or invalid
    pub fn set_valid(&mut self, tx_id: &Hash, valid: bool) -> Result<(), CoreError> {
        if !self.nodes.contains_key(tx_id) {
            return Err(CoreError::TransactionError("Transaction not found".to_string()));
        }

        self.valid_txs.insert(tx_id.clone(), valid);

        // If marking invalid, also mark children as invalid
        if !valid {
            let children: Vec<Hash> = self
                .nodes
                .get(tx_id)
                .map(|n| n.children.clone().into_iter().collect())
                .unwrap_or_default();

            for child_id in children {
                self.set_valid(&child_id, false)?;
            }
        }

        Ok(())
    }

    /// Check if transaction is valid
    pub fn is_valid(&self, tx_id: &Hash) -> bool {
        self.valid_txs.get(tx_id).copied().unwrap_or(false)
    }

    /// Check if all parent transactions are valid and exist
    pub fn parents_satisfied(&self, tx_id: &Hash) -> Result<bool, CoreError> {
        let node = self
            .get_tx(tx_id)
            .ok_or_else(|| CoreError::TransactionError("Transaction not found".to_string()))?;

        // Coinbase has no parents
        if node.is_coinbase() {
            return Ok(true);
        }

        // All parents must exist and be valid
        for parent_id in &node.parents {
            if !self.is_valid(parent_id) {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Get all valid transactions (simple list)
    pub fn get_valid_txs(&self) -> Vec<&TxNode> {
        self.nodes
            .values()
            .filter(|n| self.is_valid(&n.tx_id))
            .collect()
    }

    /// Get transactions ready for selection (all parents satisfied)
    pub fn get_ready_txs(&self) -> Result<Vec<&TxNode>, CoreError> {
        let mut ready = Vec::new();
        for node in self.nodes.values() {
            if self.is_valid(&node.tx_id) && self.parents_satisfied(&node.tx_id)? {
                ready.push(node);
            }
        }
        Ok(ready)
    }

    /// Get the number of transactions in the graph
    pub fn tx_count(&self) -> usize {
        self.nodes.len()
    }

    /// Check for cycles (should never happen with proper validation)
    pub fn has_cycles(&self) -> Result<bool, CoreError> {
        for tx_id in self.nodes.keys() {
            if self.has_cycle_from(tx_id)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// DFS to detect cycle from a given node
    fn has_cycle_from(&self, tx_id: &Hash) -> Result<bool, CoreError> {
        let mut visited = std::collections::HashSet::new();
        let mut rec_stack = std::collections::HashSet::new();
        self.visit_cycle(tx_id, &mut visited, &mut rec_stack)
    }

    fn visit_cycle(
        &self,
        tx_id: &Hash,
        visited: &mut std::collections::HashSet<Hash>,
        rec_stack: &mut std::collections::HashSet<Hash>,
    ) -> Result<bool, CoreError> {
        visited.insert(tx_id.clone());
        rec_stack.insert(tx_id.clone());

        if let Some(node) = self.nodes.get(tx_id) {
            for child_id in &node.children {
                if !visited.contains(child_id) {
                    if self.visit_cycle(child_id, visited, rec_stack)? {
                        return Ok(true);
                    }
                } else if rec_stack.contains(child_id) {
                    return Ok(true);
                }
            }
        }

        rec_stack.remove(tx_id);
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::transaction::{Transaction, TxInput, TxOutput};

    fn create_test_tx(id: &[u8], input_count: usize) -> Transaction {
        let mut inputs = vec![];
        for i in 0..input_count {
            inputs.push(TxInput {
                prev_tx: Hash::new(&[i as u8]),
                index: 0,
                signature: vec![],
                pubkey: vec![],
            });
        }

        Transaction {
            id: Hash::new(id),
            inputs,
            outputs: vec![TxOutput {
                value: 100,
                pubkey_hash: Hash::new(b"dest"),
            }],
        }
    }

    #[test]
    fn test_graph_add_transaction() {
        let mut graph = TxGraph::new();
        let tx = create_test_tx(b"tx1", 0);
        let node = TxNode::new(tx, 100);

        assert!(graph.add_tx(node).is_ok());
        assert_eq!(graph.tx_count(), 1);
    }

    #[test]
    fn test_graph_duplicate_transaction() {
        let mut graph = TxGraph::new();
        let tx = create_test_tx(b"tx1", 0);
        let node = TxNode::new(tx.clone(), 100);

        graph.add_tx(node).ok();

        let node2 = TxNode::new(tx, 100);
        assert!(graph.add_tx(node2).is_err());
    }

    #[test]
    fn test_graph_transaction_validity() {
        let mut graph = TxGraph::new();
        let tx = create_test_tx(b"tx1", 0);
        let node = TxNode::new(tx, 100);

        graph.add_tx(node).ok();
        assert!(graph.is_valid(&Hash::new(b"tx1")));
    }

    #[test]
    fn test_graph_parents_satisfied() {
        let mut graph = TxGraph::new();
        let parent_tx = create_test_tx(b"parent", 0);
        let parent_node = TxNode::new(parent_tx, 50);

        graph.add_tx(parent_node).ok();

        // Child with parent dependency
        let child_tx = Transaction {
            id: Hash::new(b"child"),
            inputs: vec![TxInput {
                prev_tx: Hash::new(b"parent"),
                index: 0,
                signature: vec![],
                pubkey: vec![],
            }],
            outputs: vec![TxOutput {
                value: 100,
                pubkey_hash: Hash::new(b"dest"),
            }],
        };

        let child_node = TxNode::new(child_tx, 50);
        graph.add_tx(child_node).ok();

        assert!(graph.parents_satisfied(&Hash::new(b"child")).ok().unwrap_or(false));
    }

    #[test]
    fn test_graph_remove_transaction() {
        let mut graph = TxGraph::new();
        let tx = create_test_tx(b"tx1", 0);
        let node = TxNode::new(tx, 100);

        graph.add_tx(node).ok();
        assert_eq!(graph.tx_count(), 1);

        graph.remove_tx(&Hash::new(b"tx1")).ok();
        assert_eq!(graph.tx_count(), 0);
    }
}
