/// Transaction node in the mempool's fee-based DAG
///
/// Represents a single transaction with:
/// - Fee information for prioritization
/// - Parent/child relationships in the transaction DAG
/// - Validation status

use crate::core::crypto::Hash;
use crate::core::state::transaction::Transaction;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct TxNode {
    /// Transaction hash
    pub tx_id: Hash,
    /// Full transaction data
    pub tx: Transaction,
    /// Total fees in satoshis
    pub fee: u64,
    /// Fee rate in satoshis per byte (ceil)
    pub fee_rate: u64,
    /// Arrival order / time for deterministic tie-breaks
    pub arrival_time: u64,
    /// Parent transactions (dependencies)
    pub parents: HashSet<Hash>,
    /// Child transactions (dependents)
    pub children: HashSet<Hash>,
}

impl TxNode {
    /// Create a new transaction node
    pub fn new(tx: Transaction, fee: u64) -> Self {
        Self::with_arrival(tx, fee, 0)
    }

    pub fn with_arrival(tx: Transaction, fee: u64, arrival_time: u64) -> Self {
        let tx_id = tx.id.clone();

        // Calculate fee_rate (ceil(fee/size)) to avoid zero fee_rate for small fees
        let size = (tx.inputs.len() * 148 + tx.outputs.len() * 34 + 10) as u64;
        let fee_rate = if size > 0 {
            (fee + size - 1).saturating_div(size)
        } else {
            0
        };

        // Extract parent transactions (inputs)
        let mut parents = HashSet::new();
        for input in &tx.inputs {
            parents.insert(input.prev_tx.clone());
        }

        Self {
            tx_id,
            tx,
            fee,
            fee_rate,
            arrival_time,
            parents,
            children: HashSet::new(),
        }
    }

    /// Add a child transaction
    pub fn add_child(&mut self, child_id: Hash) {
        self.children.insert(child_id);
    }

    /// Remove a child transaction
    pub fn remove_child(&self, child_id: &Hash) -> HashSet<Hash> {
        let mut new_children = self.children.clone();
        new_children.remove(child_id);
        new_children
    }

    /// Check if this transaction is a coinbase (no inputs)
    pub fn is_coinbase(&self) -> bool {
        self.tx.is_coinbase()
    }

    /// Get transaction size in bytes (approximate)
    pub fn size_bytes(&self) -> u64 {
        (self.tx.inputs.len() * 148 + self.tx.outputs.len() * 34 + 10) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::transaction::{TxInput, TxOutput};

    #[test]
    fn test_txnode_creation() {
        let tx = Transaction {
            id: Hash::new(b"tx1"),
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

        let node = TxNode::new(tx, 50);
        assert_eq!(node.fee, 50);
        assert!(node.fee_rate > 0);
        assert_eq!(node.parents.len(), 1);
        assert_eq!(node.children.len(), 0);
    }

    #[test]
    fn test_txnode_fee_rate() {
        let tx = Transaction {
            id: Hash::new(b"tx1"),
            inputs: vec![],
            outputs: vec![],
        };

        let node = TxNode::new(tx, 1000);
        assert!(node.fee_rate > 0);
        assert_eq!(node.size_bytes(), 10);
    }

    #[test]
    fn test_coinbase_detection() {
        let coinbase_tx = Transaction {
            id: Hash::new(b"coinbase"),
            inputs: vec![],
            outputs: vec![TxOutput {
                value: 100,
                pubkey_hash: Hash::new(b"miner"),
            }],
        };

        let node = TxNode::new(coinbase_tx, 0);
        assert!(node.is_coinbase());
    }
}
