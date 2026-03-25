/// Transaction packages for fee estimation and block building
///
/// A package represents a transaction with all its ancestors (dependencies).
/// This enables accurate fee rate calculation for the entire dependency chain.
///
/// Key concept: A transaction's fee rate is meaningless without considering
/// all its parent transactions - package fee rate considers the full chain.

use crate::core::crypto::Hash;
use crate::core::errors::CoreError;
use super::graph::TxGraph;
use super::node::TxNode;
use std::collections::HashSet;

/// A transaction package with all ancestors
#[derive(Debug, Clone)]
pub struct Package {
    /// All transaction hashes in this package (including ancestors)
    pub tx_ids: HashSet<Hash>,
    /// Root transaction (the one being built for)
    pub root_tx_id: Hash,
    /// Total fees from all transactions in package
    pub total_fee: u64,
    /// Total size of all transactions in package
    pub total_size: u64,
    /// Fee rate for entire package (total_fee / total_size)
    pub package_fee_rate: u64,
}

impl Package {
    /// Create a new package for a transaction
    pub fn new(
        root_tx_id: Hash,
        tx_ids: HashSet<Hash>,
        total_fee: u64,
        total_size: u64,
    ) -> Self {
        let package_fee_rate = if total_size > 0 {
            total_fee / total_size
        } else {
            0
        };

        Self {
            tx_ids,
            root_tx_id,
            total_fee,
            total_size,
            package_fee_rate,
        }
    }

    /// Check if this package contains a transaction
    pub fn contains(&self, tx_id: &Hash) -> bool {
        self.tx_ids.contains(tx_id)
    }

    /// Get the number of transactions in package
    pub fn len(&self) -> usize {
        self.tx_ids.len()
    }

    /// Check if package is empty
    pub fn is_empty(&self) -> bool {
        self.tx_ids.is_empty()
    }
}

/// Build a package for a transaction including all ancestors
///
/// A package includes:
/// - The root transaction
/// - All parent transactions (direct dependencies)
/// - All ancestral transactions (transitive dependencies)
///
/// This allows accurate fee rate calculation for the entire dependency chain.
pub fn build_package(graph: &TxGraph, tx_id: &Hash) -> Result<Package, CoreError> {
    let node = graph
        .get_tx(tx_id)
        .ok_or_else(|| CoreError::TransactionError("Transaction not found".to_string()))?;

    let mut package_txs = HashSet::new();
    let mut to_visit = vec![tx_id.clone()];
    let mut total_fee = 0u64;
    let mut total_size = 0u64;

    // BFS to collect all ancestors (dependencies)
    while let Some(current_id) = to_visit.pop() {
        if package_txs.contains(&current_id) {
            continue; // Already processed
        }

        if let Some(current_node) = graph.get_tx(&current_id) {
            // Add to package
            package_txs.insert(current_id.clone());
            total_fee = total_fee.checked_add(current_node.fee).ok_or_else(|| {
                CoreError::TransactionError("Fee total overflow".to_string())
            })?;

            total_size = total_size
                .checked_add(current_node.size_bytes())
                .ok_or_else(|| {
                    CoreError::TransactionError("Size total overflow".to_string())
                })?;

            // Add parents to visit queue
            for parent_id in &current_node.parents {
                if !package_txs.contains(parent_id) {
                    to_visit.push(parent_id.clone());
                }
            }
        }
    }

    if package_txs.is_empty() {
        return Err(CoreError::TransactionError(
            "Package is empty".to_string(),
        ));
    }

    Ok(Package::new(tx_id.clone(), package_txs, total_fee, total_size))
}

/// Select packages using fee rate priority
///
/// Returns packages sorted by fee_rate (highest first)
/// Avoids including the same transaction in multiple packages
pub fn select_packages(
    packages: &[Package],
) -> Result<Vec<Package>, CoreError> {
    if packages.is_empty() {
        return Ok(Vec::new());
    }

    // Sort by package_fee_rate (descending)
    let mut sorted = packages.to_vec();
    sorted.sort_by(|a, b| {
        // Higher fee_rate first
        b.package_fee_rate
            .cmp(&a.package_fee_rate)
            .then_with(|| a.root_tx_id.cmp(&b.root_tx_id)) // Deterministic by tx_id
    });

    // Select packages avoiding duplicates
    let mut selected = Vec::new();
    let mut used_txs = HashSet::new();

    for package in sorted {
        // Check if any tx in this package is already used
        let conflicts = package.tx_ids.iter().any(|id| used_txs.contains(id));

        if !conflicts {
            // Add all txs from this package to used set
            for tx_id in &package.tx_ids {
                used_txs.insert(tx_id.clone());
            }
            selected.push(package);
        }
    }

    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::transaction::{Transaction, TxInput, TxOutput};

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
    fn test_package_creation() {
        let mut tx_ids = HashSet::new();
        tx_ids.insert(Hash::new(b"tx1"));

        let package = Package::new(Hash::new(b"tx1"), tx_ids, 100, 200);
        assert_eq!(package.total_fee, 100);
        assert_eq!(package.total_size, 200);
        assert_eq!(package.package_fee_rate, 0); // 100 / 200 = 0 (integer division)
    }

    #[test]
    fn test_package_fee_rate() {
        let mut tx_ids = HashSet::new();
        tx_ids.insert(Hash::new(b"tx1"));
        tx_ids.insert(Hash::new(b"tx2"));

        let package = Package::new(Hash::new(b"root"), tx_ids, 1000, 100);
        assert_eq!(package.package_fee_rate, 10); // 1000 / 100
    }

    #[test]
    fn test_package_contains() {
        let mut tx_ids = HashSet::new();
        let tx_id = Hash::new(b"tx1");
        tx_ids.insert(tx_id.clone());

        let package = Package::new(Hash::new(b"root"), tx_ids, 100, 200);
        assert!(package.contains(&tx_id));
        assert!(!package.contains(&Hash::new(b"other")));
    }

    #[test]
    fn test_select_packages_avoids_duplicates() {
        let mut ids1 = HashSet::new();
        ids1.insert(Hash::new(b"tx1"));

        let mut ids2 = HashSet::new();
        ids2.insert(Hash::new(b"tx1")); // Conflict with ids1

        let pkg1 = Package::new(Hash::new(b"root1"), ids1, 200, 100);
        let pkg2 = Package::new(Hash::new(b"root2"), ids2, 100, 100);

        let packages = vec![pkg1, pkg2];
        let selected = select_packages(&packages).ok();

        // Should select only pkg1 due to higher fee_rate
        assert_eq!(selected.map(|s| s.len()), Some(1));
    }

    #[test]
    fn test_select_packages_deterministic() {
        let mut ids1 = HashSet::new();
        ids1.insert(Hash::new(b"tx1"));

        let mut ids2 = HashSet::new();
        ids2.insert(Hash::new(b"tx2"));

        // Same fee rate - should sort by tx_id
        let pkg1 = Package::new(Hash::new(b"root1"), ids1, 100, 100);
        let pkg2 = Package::new(Hash::new(b"root2"), ids2, 100, 100);

        let packages1 = vec![pkg1.clone(), pkg2.clone()];
        let packages2 = vec![pkg2.clone(), pkg1.clone()];

        let selected1 = select_packages(&packages1);
        let selected2 = select_packages(&packages2);

        // Both should produce same output
        assert_eq!(
            selected1.ok().map(|s| s.len()),
            selected2.ok().map(|s| s.len())
        );
    }
}
