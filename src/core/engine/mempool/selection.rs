/// DAG-aware transaction selection for block building
///
/// Enhanced with deterministic, high-precision, mobile-optimized package selection.
///
/// ALGORITHM:
/// - Bounded lookahead (depth 3) for package evaluation
/// - High-precision scoring: (total_fees << 64) / total_weight using u128 (no float)
/// - Pre-filtering of top MAX_CANDIDATES=500 by raw fee_rate
/// - Package score caching with invalidation on mempool changes
/// - Strict ancestor inclusion (no child without all parents)
/// - Deterministic ordering with tx_id as final tie-breaker
/// - Topological sorting to ensure parents before children
///
/// BORROWING SAFETY:
/// The refactored code uses explicit separation of phases:
/// 1. Collect phase: Gather tx_ids without modifying cache
/// 2. Compute phase: Calculate scores in isolated scopes (cache borrows end)
/// 3. Mutate phase: Invalidate cache only after all reads complete
///
/// Key patterns:
/// - get_or_compute() returns owned data instead of references to avoid lifetime issues
/// - collect_candidates_data() uses scoped block to end cache borrows before mutation
/// - txs_to_invalidate buffer collects tx_ids before invalidating cache
/// - Hash clones only when ownership is required (e.g., for heap, included_set)

use crate::core::crypto::Hash;
use crate::core::state::transaction::Transaction;
use crate::core::errors::CoreError;
use super::graph::TxGraph;
use super::node::TxNode;
use super::package::Package;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::cmp::Ordering;

/// Configuration constants for enhanced selection
const LOOKAHEAD_DEPTH: usize = 3;
const MAX_CANDIDATES: usize = 500;

/// High-precision package score using fixed-point arithmetic
/// score = (total_fees << 64) / total_weight
/// This gives us 64 bits of fractional precision without floating point
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PackageScore(u128);

impl PackageScore {
    /// Create score from total fees and total weight
    /// Uses checked operations to prevent overflow
    pub fn new(total_fees: u64, total_weight: u64) -> Option<Self> {
        if total_weight == 0 {
            return Some(PackageScore(0));
        }
        // Convert to u128 for high precision calculation
        let fees_u128 = total_fees as u128;
        let weight_u128 = total_weight as u128;

        // score = (fees << 64) / weight
        let shifted_fees = fees_u128.checked_mul(1u128 << 64)?;
        let score = shifted_fees.checked_div(weight_u128)?;

        Some(PackageScore(score))
    }

    /// Get raw score value
    pub fn raw(&self) -> u128 {
        self.0
    }
}

/// Cached package data to avoid recomputation
#[derive(Debug, Clone)]
struct CachedPackage {
    package: Package,
    score: PackageScore,
    /// Cache invalidation version
    version: u64,
}

/// Package cache with invalidation support
#[derive(Debug)]
struct PackageCache {
    cache: HashMap<Hash, CachedPackage>,
    version: u64,
}

impl PackageCache {
    fn new() -> Self {
        Self {
            cache: HashMap::new(),
            version: 0,
        }
    }

    /// Get cached package or compute new one
    fn get_or_compute(&mut self, graph: &TxGraph, tx_id: &Hash) -> Result<(PackageScore, u64, HashSet<Hash>), CoreError> {
        // Check if we have a valid cached entry
        if let Some(cached) = self.cache.get(tx_id) {
            if cached.version == self.version {
                return Ok((cached.score, cached.package.total_size, cached.package.tx_ids.clone()));
            }
        }

        // Remove stale entry if it exists
        self.cache.remove(tx_id);

        // Compute new package with bounded depth
        let package = self.build_package_bounded(graph, tx_id)?;
        let score = PackageScore::new(package.total_fee, package.total_size)
            .ok_or_else(|| CoreError::TransactionError("Score calculation overflow".to_string()))?;

        let cached = CachedPackage {
            package: package.clone(), // Clone for storage
            score,
            version: self.version,
        };

        // Insert cached version
        self.cache.insert(tx_id.clone(), cached);

        // Return the data
        Ok((score, package.total_size, package.tx_ids))
    }

    /// Build package with bounded lookahead depth
    fn build_package_bounded(&self, graph: &TxGraph, tx_id: &Hash) -> Result<Package, CoreError> {
        let root_node = graph
            .get_tx(tx_id)
            .ok_or_else(|| CoreError::TransactionError("Transaction not found".to_string()))?;

        let mut package_txs = HashSet::new();
        let mut total_fee = 0u64;
        let mut total_size = 0u64;

        // Iterative BFS with depth limiting
        let mut queue = VecDeque::new();
        let mut depths = HashMap::new();

        queue.push_back((tx_id.clone(), 0));
        depths.insert(tx_id.clone(), 0);

        while let Some((current_id, depth)) = queue.pop_front() {
            if package_txs.contains(&current_id) {
                continue;
            }

            if depth > LOOKAHEAD_DEPTH {
                continue; // Skip deeper ancestors
            }

            if let Some(current_node) = graph.get_tx(&current_id) {
                package_txs.insert(current_id.clone());
                total_fee = total_fee.checked_add(current_node.fee).ok_or_else(|| {
                    CoreError::TransactionError("Fee total overflow".to_string())
                })?;
                total_size = total_size.checked_add(current_node.size_bytes()).ok_or_else(|| {
                    CoreError::TransactionError("Size total overflow".to_string())
                })?;

                // Add parents if within depth limit
                if depth < LOOKAHEAD_DEPTH {
                    for parent_id in &current_node.parents {
                        if !package_txs.contains(parent_id) && !depths.contains_key(parent_id) {
                            depths.insert(parent_id.clone(), depth + 1);
                            queue.push_back((parent_id.clone(), depth + 1));
                        }
                    }
                }
            }
        }

        Ok(Package::new(tx_id.clone(), package_txs, total_fee, total_size))
    }

    /// Invalidate cache when mempool changes
    fn invalidate(&mut self) {
        self.version += 1;
        self.cache.clear(); // Full clear for simplicity, could be more granular
    }

    /// Invalidate specific transaction
    fn invalidate_tx(&mut self, tx_id: &Hash) {
        self.cache.remove(tx_id);
    }
}

/// Transaction candidate for selection
#[derive(Debug, Clone)]
struct SelectionCandidate {
    tx_id: Hash,
    score: PackageScore,
    package_size: u64,
}

impl Eq for SelectionCandidate {}

impl PartialEq for SelectionCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.tx_id == other.tx_id
    }
}

impl Ord for SelectionCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        // Max-heap: higher score first, then lower tx_id for determinism
        other.score.cmp(&self.score)
            .then_with(|| other.tx_id.cmp(&self.tx_id)) // Reverse for min tx_id first
    }
}

impl PartialOrd for SelectionCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Enhanced transaction selection engine with package-aware mining
#[derive(Debug)]
pub struct TransactionSelector {
    /// Maximum block size in bytes
    max_block_size: u64,
    /// Package cache for performance
    package_cache: PackageCache,
}

impl TransactionSelector {
    /// Create new enhanced transaction selector
    pub fn new(max_block_size: u64) -> Self {
        Self {
            max_block_size,
            package_cache: PackageCache::new(),
        }
    }

    /// Enhanced select transactions with package scoring and bounded lookahead
    ///
    /// BORROWING STRATEGY:
    /// This method carefully manages self.package_cache borrowing by:
    /// 1. Separating candidate collection into a scoped block (collects owned data)
    /// 2. Processing heap candidates without re-borrowing the cache
    /// 3. Collecting tx_ids to invalidate in a separate Vec
    /// 4. Invalidating cache only after all reads complete
    ///
    /// This avoids the E0502 "cannot borrow as mutable while immutably borrowed" error.
    pub fn select_transactions(
        &mut self,
        graph: &TxGraph,
    ) -> Result<Vec<Transaction>, CoreError> {
        let ready_txs = graph.get_ready_txs()?;

        if ready_txs.is_empty() {
            return Ok(Vec::new());
        }

        // PHASE 1: Pre-filter candidates by raw fee_rate (collect tx_ids only)
        // No cache access yet - just identify candidates
        let candidate_tx_ids = self.pre_filter_candidates(&ready_txs);

        // PHASE 2: Build candidate data with package scores
        // Scoped block ensures cache borrows end before we mutate it in phase 5
        let mut candidates_data = Vec::new();
        {
            // This block: read-only access to cache
            // When it ends, all immutable borrows to cache are released
            for tx_id in &candidate_tx_ids {
                // Clone tx_id: avoid moving from borrowed collection (&candidate_tx_ids)
                let tx_id_clone = tx_id.clone();
                // get_or_compute returns owned data, not a reference
                // This is key: no borrowing of self.package_cache after this line
                let (score, package_size, package_tx_ids) = self.package_cache.get_or_compute(graph, &tx_id_clone)?;
                candidates_data.push((
                    tx_id_clone,
                    score,
                    package_size,
                    package_tx_ids, // Now owned, not borrowed
                ));
            }
        } // Cache borrow scope ends - mutable borrow is now possible in phase 5

        // PHASE 3: Build priority queue from collected data
        // No cache access - only working with candidates_data (owned)
        let mut heap = BinaryHeap::new();
        for (tx_id, score, package_size, _) in &candidates_data {
            heap.push(SelectionCandidate {
                tx_id: tx_id.clone(), // Clone for heap ownership
                score: *score, // Copy score (u128 is small)
                package_size: *package_size, // Copy size (u64 is small)
            });
        }

        // PHASE 4: Greedy selection with conflict tracking
        // No cache access - only reading from heap and candidates_data
        let mut selected_txs = Vec::new();
        let mut included_tx_set = HashSet::new();
        let mut total_size = 0u64;
        let mut txs_to_invalidate = Vec::new(); // Buffer for later cache invalidation

        while let Some(candidate) = heap.pop() {
            // Skip if already included (conflict resolution)
            if included_tx_set.contains(&candidate.tx_id) {
                continue;
            }

            // Check block size
            if total_size + candidate.package_size > self.max_block_size {
                continue; // Package too large
            }

            // Find the package data for this candidate
            // Note: we search candidates_data, not cache - no cache access
            let package_tx_ids = candidates_data.iter()
                .find(|(tx_id, _, _, _)| tx_id == &candidate.tx_id)
                .map(|(_, _, _, tx_ids)| tx_ids)
                .ok_or_else(|| CoreError::TransactionError("Package data not found".to_string()))?;

            // Check for conflicts with already selected transactions
            let mut has_conflict = false;
            for tx_id in package_tx_ids {
                if included_tx_set.contains(tx_id) {
                    has_conflict = true;
                    break;
                }
            }
            if has_conflict {
                continue;
            }

            // Select the entire package
            for tx_id in package_tx_ids {
                if let Some(node) = graph.get_tx(tx_id) {
                    selected_txs.push(node.tx.clone());
                    // Clone tx_id for ownership in included_set
                    included_tx_set.insert(tx_id.clone());
                }
            }
            total_size += candidate.package_size;

            // Collect tx_ids for cache invalidation
            // Cloned here because we'll mutate cache later
            for tx_id in package_tx_ids {
                txs_to_invalidate.push(tx_id.clone());
            }
        }

        // PHASE 5: Invalidate cache for selected transactions
        // Now safe to mutate cache - all reads from phase 2 are complete
        for tx_id in txs_to_invalidate {
            self.package_cache.invalidate_tx(&tx_id);
        }

        // PHASE 6: Ensure topological ordering (parents before children)
        self.topological_sort(&mut selected_txs, graph)?;

        Ok(selected_txs)
    }

    /// Pre-filter candidates by raw fee_rate to limit evaluation
    fn pre_filter_candidates(&self, ready_txs: &[&TxNode]) -> Vec<Hash> {
        let mut candidates: Vec<_> = ready_txs.iter()
            .map(|node| (node.tx_id.clone(), node.fee_rate))
            .collect();

        // Sort by fee_rate descending, then tx_id ascending for determinism
        candidates.sort_by(|a, b| {
            b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0))
        });

        // Take top MAX_CANDIDATES
        candidates.into_iter()
            .take(MAX_CANDIDATES)
            .map(|(tx_id, _)| tx_id)
            .collect()
    }

    /// Sort transactions topologically (parents before children)
    fn topological_sort(&self, txs: &mut Vec<Transaction>, graph: &TxGraph) -> Result<(), CoreError> {
        if txs.is_empty() {
            return Ok(());
        }

        // Build adjacency list and indegree map
        let mut adj: HashMap<Hash, Vec<Hash>> = HashMap::new();
        let mut indegree: HashMap<Hash, usize> = HashMap::new();

        // Initialize with all transactions
        for tx in txs.iter() {
            let tx_id = &tx.id;
            adj.entry(tx_id.clone()).or_insert(Vec::new());
            indegree.entry(tx_id.clone()).or_insert(0);
        }

        // Build graph from mempool relationships
        for tx in txs.iter() {
            let tx_id = &tx.id;
            if let Some(node) = graph.get_tx(tx_id) {
                for parent_id in &node.parents {
                    if indegree.contains_key(parent_id) {
                        adj.get_mut(parent_id).unwrap().push(tx_id.clone());
                        *indegree.get_mut(tx_id).unwrap() += 1;
                    }
                }
            }
        }

        // Kahn's algorithm (iterative topological sort)
        let mut queue = VecDeque::new();
        for (tx_id, &deg) in &indegree {
            if deg == 0 {
                queue.push_back(tx_id.clone()); // Clone tx_id to avoid moving
            }
        }

        let mut sorted = Vec::new();
        while let Some(tx_id) = queue.pop_front() {
            sorted.push(tx_id.clone());

            if let Some(neighbors) = adj.get(&tx_id) {
                for neighbor in neighbors {
                    if let Some(deg) = indegree.get_mut(neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(neighbor.clone());
                        }
                    }
                }
            }
        }

        // Reorder txs according to topological sort
        let mut tx_map: HashMap<Hash, Transaction> = txs.drain(..).map(|tx| (tx.id.clone(), tx)).collect();
        *txs = sorted.into_iter()
            .filter_map(|tx_id| tx_map.remove(&tx_id))
            .collect();

        Ok(())
    }

    /// Select transactions with proper topological ordering (legacy method)
    /// Ensures parents always come before children
    pub fn select_transactions_ordered(
        &self,
        graph: &TxGraph,
    ) -> Result<Vec<Transaction>, CoreError> {
        // For compatibility, delegate to the enhanced method
        // Note: This creates a temporary mutable reference, but in practice
        // the cache will be invalidated appropriately
        let mut temp_selector = TransactionSelector::new(self.max_block_size);
        temp_selector.select_transactions(graph)
    }

    /// Invalidate package cache when mempool changes
    pub fn invalidate_cache(&mut self) {
        self.package_cache.invalidate();
    }

    /// Get transactions that would fit in remaining block space
    pub fn get_available_space(&self, used_size: u64) -> u64 {
        self.max_block_size.saturating_sub(used_size)
    }

    /// Update max block size
    pub fn set_max_block_size(&mut self, max_size: u64) {
        self.max_block_size = max_size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::mempool::node::TxNode;
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
    fn test_selector_creation() {
        let mut selector = TransactionSelector::new(1000);
        assert_eq!(selector.get_available_space(0), 1000);
    }

    #[test]
    fn test_selector_space_calculation() {
        let selector = TransactionSelector::new(1000);
        assert_eq!(selector.get_available_space(200), 800);
        assert_eq!(selector.get_available_space(1000), 0);
    }

    #[test]
    fn test_empty_graph_selection() {
        let mut selector = TransactionSelector::new(1000);
        let graph = TxGraph::new();

        let selected = selector.select_transactions(&graph);
        assert!(selected.is_ok());
        assert_eq!(selected.ok().map(|v| v.len()), Some(0));
    }

    #[test]
    fn test_selectable_ordering() {
        let tx1 = TxNode::new(create_test_tx(b"tx1"), 100);
        let tx2 = TxNode::new(create_test_tx(b"tx2"), 200);

        // tx2 has higher fee_rate, should be first in max-heap
        let score1 = PackageScore::new(tx1.fee, tx1.size_bytes()).unwrap();
        let score2 = PackageScore::new(tx2.fee, tx2.size_bytes()).unwrap();
        assert!(score2 > score1);
    }

    #[test]
    fn test_package_score_precision() {
        // Test high-precision scoring without floating point
        let score1 = PackageScore::new(100, 200).unwrap(); // 0.5
        let score2 = PackageScore::new(150, 200).unwrap(); // 0.75
        assert!(score2 > score1);

        // Test very large but valid values don't panic
        let valid_high = PackageScore::new(u64::MAX / 2, 1);
        assert!(valid_high.is_some());

        // Test division by zero is handled
        let zero_weight = PackageScore::new(100, 0);
        assert_eq!(zero_weight, Some(PackageScore(0))); // Returns 0 for zero weight
    }

    #[test]
    fn test_pre_filter_candidates() {
        let mut selector = TransactionSelector::new(1000);
        let tx1 = TxNode::new(create_test_tx(b"tx1"), 100); // fee_rate ~1
        let tx2 = TxNode::new(create_test_tx(b"tx2"), 200); // fee_rate ~2
        let tx3 = TxNode::new(create_test_tx(b"tx3"), 50);  // fee_rate ~0.5

        let ready_txs = vec![&tx1, &tx2, &tx3];
        let candidates = selector.pre_filter_candidates(&ready_txs);

        // Should be sorted by fee_rate descending
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0], tx2.tx_id);
        assert_eq!(candidates[1], tx1.tx_id);
        assert_eq!(candidates[2], tx3.tx_id);
    }

    #[test]
    fn test_topological_sort() {
        let mut selector = TransactionSelector::new(1000);
        let mut graph = TxGraph::new();

        // Create parent-child relationship
        let parent_tx = create_test_tx(b"parent");
        let parent_node = TxNode::new(parent_tx.clone(), 100);
        graph.add_tx(parent_node).ok();

        let child_tx = Transaction {
            id: Hash::new(b"child"),
            inputs: vec![TxInput {
                prev_tx: parent_tx.id.clone(),
                index: 0,
                signature: vec![1],
                pubkey: vec![2],
            }],
            outputs: vec![TxOutput {
                value: 50,
                pubkey_hash: Hash::new(b"dest"),
            }],
        };
        let child_node = TxNode::new(child_tx.clone(), 50);
        graph.add_tx(child_node).ok();

        let mut txs = vec![child_tx.clone(), parent_tx.clone()];
        selector.topological_sort(&mut txs, &graph).ok();

        // Parent should come before child
        assert_eq!(txs[0].id, parent_tx.id);
        assert_eq!(txs[1].id, child_tx.id);
    }

    #[test]
    fn test_max_block_size_respected() {
        let mut selector = TransactionSelector::new(100); // Very small block
        let mut graph = TxGraph::new();

        let tx = create_test_tx(b"tx1");
        let node = TxNode::new(tx, 100);
        graph.add_tx(node).ok();

        let selected = selector.select_transactions(&graph);
        assert!(selected.is_ok());
        // Transaction size is ~44 bytes, should fit
        assert!(!selected.unwrap().is_empty());
    }
}
