use crate::core::dag::BlockNode;
use crate::core::state::storage::Storage;
use crate::core::state::transaction::Transaction;
use crate::core::state::utxo::UtxoSet;
use crate::core::state::v_trie::VerkleTree;

/// Minimal state container exposing the current Verkle root.
#[derive(Debug, Clone)]
pub struct State {
    pub root: [u8; 32],
}

/// Snapshot of the chain state at a specific block height.
#[derive(Debug, Clone)]
pub struct StateSnapshot {
    pub height: u64,
    pub root: [u8; 32],
}

/// Basic state manager for applying blocks, tracking snapshots, and rolling back.
#[derive(Debug)]
pub struct StateManager<S: Storage + Clone> {
    pub tree: VerkleTree<S>,
    pub current_height: u64,
    pub snapshots: Vec<StateSnapshot>,
    snapshot_storages: Vec<S>,
}

impl<S: Storage + Clone> StateManager<S> {
    pub fn new(tree: VerkleTree<S>) -> Self {
        let root = tree.get_root();
        let storage_snapshot = tree.storage_clone();

        Self {
            tree,
            current_height: 0,
            snapshots: vec![StateSnapshot { height: 0, root }],
            snapshot_storages: vec![storage_snapshot],
        }
    }

    pub fn apply_block(&mut self, block: &BlockNode, utxo: &mut UtxoSet) {
        for tx in &block.transactions {
            self.apply_transaction(tx, utxo);
        }

        self.current_height += 1;

        let new_root = self.tree.get_root();
        self.snapshots.push(StateSnapshot {
            height: self.current_height,
            root: new_root,
        });
        self.snapshot_storages.push(self.tree.storage_clone());
    }

    fn apply_transaction(&mut self, tx: &Transaction, utxo: &mut UtxoSet) {
        for input in &tx.inputs {
            let key = (input.prev_tx.clone(), input.index);
            utxo.utxos.remove(&key);
        }

        for (i, output) in tx.outputs.iter().enumerate() {
            let key = tx.hash_with_index(i as u32);
            utxo.utxos.insert((tx.id.clone(), i as u32), output.clone());
            self.tree.insert(key, output.serialize());
        }
    }

    pub fn get_state_at(&self, height: u64) -> Option<&StateSnapshot> {
        self.snapshots.iter().find(|s| s.height == height)
    }

    pub fn rollback(&mut self, target_height: u64) {
        assert!(target_height <= self.current_height);

        self.snapshots.truncate(target_height as usize + 1);
        self.snapshot_storages.truncate(target_height as usize + 1);
        self.current_height = target_height;

        let snapshot_storage = self
            .snapshot_storages
            .get(target_height as usize)
            .expect("rollback snapshot missing");

        // NOTE: VerkleTree is not fully versioned; reset to the storage snapshot.
        self.tree = VerkleTree::new(snapshot_storage.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::crypto::Hash;
    use crate::core::dag::BlockNode;
    use crate::core::state::storage::MemoryStorage;
    use crate::core::state::transaction::{TxOutput, Transaction};
    use std::collections::HashSet;

    fn make_coinbase_transaction(value: u64, pubkey_hash: Hash) -> Transaction {
        Transaction::new(
            Vec::new(),
            vec![TxOutput {
                value,
                pubkey_hash,
            }],
        )
    }

    fn make_block(id_bytes: &[u8], transactions: Vec<Transaction>) -> BlockNode {
        BlockNode {
            id: Hash::new(id_bytes),
            parents: HashSet::new(),
            children: HashSet::new(),
            selected_parent: None,
            blue_set: HashSet::new(),
            red_set: HashSet::new(),
            blue_score: 0,
            timestamp: 0,
            difficulty: 0,
            nonce: 0,
            transactions,
        }
    }

    #[test]
    fn test_state_manager_apply_block() {
        let storage = MemoryStorage::new();
        let tree = VerkleTree::new(storage);
        let mut manager = StateManager::new(tree);
        let mut utxo = UtxoSet::new();

        let tx = make_coinbase_transaction(42, Hash::new(b"alice"));
        let block = make_block(b"block-1", vec![tx.clone()]);

        let root_before = manager.tree.get_root();
        manager.apply_block(&block, &mut utxo);
        let root_after = manager.tree.get_root();

        assert_ne!(root_before, root_after);
        assert_eq!(manager.current_height, 1);
        assert_eq!(manager.snapshots.len(), 2);
        assert_eq!(utxo.utxos.len(), 1);
        assert_eq!(manager.get_state_at(1).unwrap().root, root_after);
    }

    #[test]
    fn test_state_manager_snapshot() {
        let storage = MemoryStorage::new();
        let tree = VerkleTree::new(storage);
        let mut manager = StateManager::new(tree);
        let mut utxo = UtxoSet::new();

        let block1 = make_block(b"block-1", vec![make_coinbase_transaction(10, Hash::new(b"alice"))]);
        manager.apply_block(&block1, &mut utxo);
        let snapshot1 = manager.get_state_at(1).expect("snapshot at height 1");
        let snapshot1_root = snapshot1.root;
        let snapshot1_height = snapshot1.height;

        let block2 = make_block(b"block-2", vec![make_coinbase_transaction(20, Hash::new(b"bob"))]);
        manager.apply_block(&block2, &mut utxo);
        let snapshot2 = manager.get_state_at(2).expect("snapshot at height 2");

        assert_ne!(snapshot1_root, snapshot2.root);
        assert_eq!(snapshot1_height, 1);
        assert_eq!(snapshot2.height, 2);
    }

    #[test]
    fn test_state_manager_rollback() {
        let storage = MemoryStorage::new();
        let tree = VerkleTree::new(storage);
        let mut manager = StateManager::new(tree);
        let mut utxo = UtxoSet::new();

        let block1 = make_block(b"block-1", vec![make_coinbase_transaction(10, Hash::new(b"alice"))]);
        manager.apply_block(&block1, &mut utxo);
        let root1 = manager.tree.get_root();

        let block2 = make_block(b"block-2", vec![make_coinbase_transaction(20, Hash::new(b"bob"))]);
        manager.apply_block(&block2, &mut utxo);
        let root2 = manager.tree.get_root();

        assert_ne!(root1, root2);
        assert_eq!(manager.current_height, 2);

        manager.rollback(1);

        assert_eq!(manager.current_height, 1);
        assert_eq!(manager.snapshots.len(), 2);
        assert_eq!(manager.get_state_at(1).unwrap().root, root1);
        assert_eq!(manager.tree.get_root(), root1);
    }
}
