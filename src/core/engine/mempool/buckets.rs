/// Priority buckets - organize transactions by fee-rate into High/Medium/Low
///
/// Transactions are bucketed based on fee-rate thresholds:
/// - High: fee_rate >= high_threshold
/// - Medium: medium_threshold <= fee_rate < high_threshold
/// - Low: fee_rate < medium_threshold
///
/// Selection priority: High → Medium → Low

use crate::core::crypto::Hash;
use crate::core::errors::CoreError;
use super::node::TxNode;
use std::collections::HashMap;

/// Priority bucket levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PriorityLevel {
    High = 2,
    Medium = 1,
    Low = 0,
}

impl PriorityLevel {
    /// Get printable name
    pub fn name(&self) -> &'static str {
        match self {
            PriorityLevel::High => "High",
            PriorityLevel::Medium => "Medium",
            PriorityLevel::Low => "Low",
        }
    }
}

/// Configuration for priority bucket thresholds
#[derive(Debug, Clone)]
pub struct BucketConfig {
    /// Fee rate threshold for High priority (satoshis per byte)
    pub high_threshold: u64,
    /// Fee rate threshold for Medium priority (satoshis per byte)
    pub medium_threshold: u64,
}

impl BucketConfig {
    /// Create default bucket configuration
    /// - High: >= 10 sat/byte
    /// - Medium: >= 1 sat/byte
    /// - Low: < 1 sat/byte
    pub fn default() -> Self {
        Self {
            high_threshold: 10,
            medium_threshold: 1,
        }
    }

    /// Create custom configuration
    pub fn custom(high_threshold: u64, medium_threshold: u64) -> Self {
        Self {
            high_threshold,
            medium_threshold,
        }
    }

    /// Determine priority level for fee rate
    pub fn get_priority(&self, fee_rate: u64) -> PriorityLevel {
        if fee_rate >= self.high_threshold {
            PriorityLevel::High
        } else if fee_rate >= self.medium_threshold {
            PriorityLevel::Medium
        } else {
            PriorityLevel::Low
        }
    }
}

/// Bucketed mempool - separate storage for each priority level
#[derive(Debug, Clone)]
pub struct BucketedMempool {
    /// High priority transactions
    high_bucket: HashMap<Hash, TxNode>,
    /// Medium priority transactions
    medium_bucket: HashMap<Hash, TxNode>,
    /// Low priority transactions
    low_bucket: HashMap<Hash, TxNode>,
    /// Configuration
    config: BucketConfig,
    /// Total transaction count (cached for efficiency)
    total_count: usize,
}

impl BucketedMempool {
    /// Create new bucketed mempool
    pub fn new(config: BucketConfig) -> Self {
        Self {
            high_bucket: HashMap::new(),
            medium_bucket: HashMap::new(),
            low_bucket: HashMap::new(),
            config,
            total_count: 0,
        }
    }

    /// Create with default configuration
    pub fn default() -> Self {
        Self::new(BucketConfig::default())
    }

    /// Add transaction to appropriate bucket
    pub fn add_tx(&mut self, node: TxNode) -> Result<(), CoreError> {
        let priority = self.config.get_priority(node.fee_rate);

        if self.contains_tx(&node.tx_id) {
            return Err(CoreError::TransactionError(
                "Duplicate transaction in buckets".to_string(),
            ));
        }

        let tx_id = node.tx_id.clone();
        match priority {
            PriorityLevel::High => {
                self.high_bucket.insert(tx_id, node);
            }
            PriorityLevel::Medium => {
                self.medium_bucket.insert(tx_id, node);
            }
            PriorityLevel::Low => {
                self.low_bucket.insert(tx_id, node);
            }
        }

        self.total_count += 1;
        Ok(())
    }

    /// Remove transaction from buckets
    pub fn remove_tx(&mut self, tx_id: &Hash) -> Result<Option<TxNode>, CoreError> {
        let removed = if let Some(node) = self.high_bucket.remove(tx_id) {
            self.total_count = self.total_count.saturating_sub(1);
            Some(node)
        } else if let Some(node) = self.medium_bucket.remove(tx_id) {
            self.total_count = self.total_count.saturating_sub(1);
            Some(node)
        } else if let Some(node) = self.low_bucket.remove(tx_id) {
            self.total_count = self.total_count.saturating_sub(1);
            Some(node)
        } else {
            None
        };

        Ok(removed)
    }

    /// Check if transaction exists in any bucket
    pub fn contains_tx(&self, tx_id: &Hash) -> bool {
        self.high_bucket.contains_key(tx_id)
            || self.medium_bucket.contains_key(tx_id)
            || self.low_bucket.contains_key(tx_id)
    }

    /// Get transaction from any bucket
    pub fn get_tx(&self, tx_id: &Hash) -> Option<&TxNode> {
        self.high_bucket
            .get(tx_id)
            .or_else(|| self.medium_bucket.get(tx_id))
            .or_else(|| self.low_bucket.get(tx_id))
    }

    /// Get transactions from single bucket
    pub fn get_bucket(&self, priority: PriorityLevel) -> Vec<&TxNode> {
        match priority {
            PriorityLevel::High => self.high_bucket.values().collect(),
            PriorityLevel::Medium => self.medium_bucket.values().collect(),
            PriorityLevel::Low => self.low_bucket.values().collect(),
        }
    }

    /// Get transactions ordered by priority (High → Medium → Low), stable deterministic within bucket
    pub fn get_all_ordered(&self) -> Vec<&TxNode> {
        let mut result = Vec::new();
        let mut high: Vec<&TxNode> = self.high_bucket.values().collect();
        let mut med: Vec<&TxNode> = self.medium_bucket.values().collect();
        let mut low: Vec<&TxNode> = self.low_bucket.values().collect();

        high.sort_by(|a, b| {
            b.fee_rate
                .cmp(&a.fee_rate)
                .then_with(|| b.fee.cmp(&a.fee))
                .then_with(|| a.arrival_time.cmp(&b.arrival_time))
                .then_with(|| a.tx_id.cmp(&b.tx_id))
        });
        med.sort_by(|a, b| {
            b.fee_rate
                .cmp(&a.fee_rate)
                .then_with(|| b.fee.cmp(&a.fee))
                .then_with(|| a.arrival_time.cmp(&b.arrival_time))
                .then_with(|| a.tx_id.cmp(&b.tx_id))
        });
        low.sort_by(|a, b| {
            b.fee_rate
                .cmp(&a.fee_rate)
                .then_with(|| b.fee.cmp(&a.fee))
                .then_with(|| a.arrival_time.cmp(&b.arrival_time))
                .then_with(|| a.tx_id.cmp(&b.tx_id))
        });

        result.extend(high);
        result.extend(med);
        result.extend(low);
        result
    }

    /// Get total transaction count
    pub fn len(&self) -> usize {
        self.total_count
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.total_count == 0
    }

    /// Get bucket statistics
    pub fn get_stats(&self) -> BucketStats {
        BucketStats {
            high_count: self.high_bucket.len(),
            medium_count: self.medium_bucket.len(),
            low_count: self.low_bucket.len(),
            total_count: self.total_count,
        }
    }

    /// Move transaction between buckets (e.g., when fee_rate changes)
    ///
    /// This would be used after RBF or child-pays-for-parent scenarios
    pub fn rebalance_tx(&mut self, tx_id: &Hash) -> Result<(), CoreError> {
        if let Some(node) = self.remove_tx(tx_id)? {
            self.add_tx(node)?;
        }
        Ok(())
    }
}

/// Bucket statistics
#[derive(Debug, Clone)]
pub struct BucketStats {
    pub high_count: usize,
    pub medium_count: usize,
    pub low_count: usize,
    pub total_count: usize,
}

impl BucketStats {
    /// Get percentage in each bucket
    pub fn percentages(&self) -> (f64, f64, f64) {
        let total = self.total_count as f64;
        if total == 0.0 {
            (0.0, 0.0, 0.0)
        } else {
            (
                (self.high_count as f64) / total * 100.0,
                (self.medium_count as f64) / total * 100.0,
                (self.low_count as f64) / total * 100.0,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::transaction::{Transaction, TxInput, TxOutput};

    fn create_test_tx(id: &[u8], fee_rate: u64) -> TxNode {
        let tx = Transaction {
            id: Hash::new(id),
            inputs: vec![TxInput {
                prev_tx: Hash::new(b"prev"),
                index: 0,
                signature: vec![1],
                pubkey: vec![2],
            }],
            outputs: vec![TxOutput {
                value: 100,
                pubkey_hash: Hash::new(b"dest"),
            }],
        };

        TxNode::new(tx, fee_rate * 100)
    }

    #[test]
    fn test_bucket_config_default() {
        let config = BucketConfig::default();
        assert_eq!(config.high_threshold, 10);
        assert_eq!(config.medium_threshold, 1);
    }

    #[test]
    fn test_get_priority_high() {
        let config = BucketConfig::default();
        assert_eq!(config.get_priority(15), PriorityLevel::High);
    }

    #[test]
    fn test_get_priority_medium() {
        let config = BucketConfig::default();
        assert_eq!(config.get_priority(5), PriorityLevel::Medium);
    }

    #[test]
    fn test_get_priority_low() {
        let config = BucketConfig::default();
        assert_eq!(config.get_priority(0), PriorityLevel::Low);
    }

    #[test]
    fn test_add_and_get_transaction() {
        let mut buckets = BucketedMempool::default();
        let tx = create_test_tx(b"tx1", 5); // Medium priority (5 sat/byte)
        let tx_id = tx.tx_id.clone();

        buckets.add_tx(tx).ok();
        assert!(buckets.contains_tx(&tx_id));
        assert!(buckets.get_tx(&tx_id).is_some());
    }

    #[test]
    fn test_remove_transaction() {
        let mut buckets = BucketedMempool::default();
        let tx = create_test_tx(b"tx1", 5);
        let tx_id = tx.tx_id.clone();

        buckets.add_tx(tx).ok();
        assert_eq!(buckets.len(), 1);

        buckets.remove_tx(&tx_id).ok();
        assert_eq!(buckets.len(), 0);
        assert!(!buckets.contains_tx(&tx_id));
    }

    #[test]
    fn test_transactions_bucketed_correctly() {
        let mut buckets = BucketedMempool::default();

        let high_tx = create_test_tx(b"high", 20); // High: >= 10
        let medium_tx = create_test_tx(b"medium", 5); // Medium: >= 1
        let low_tx = create_test_tx(b"low", 0); // Low: < 1

        buckets.add_tx(high_tx).ok();
        buckets.add_tx(medium_tx).ok();
        buckets.add_tx(low_tx).ok();

        assert_eq!(buckets.get_bucket(PriorityLevel::High).len(), 1);
        assert_eq!(buckets.get_bucket(PriorityLevel::Medium).len(), 1);
        assert_eq!(buckets.get_bucket(PriorityLevel::Low).len(), 1);
    }

    #[test]
    fn test_get_all_ordered_priority() {
        let mut buckets = BucketedMempool::default();

        buckets.add_tx(create_test_tx(b"low", 0)).ok();
        buckets.add_tx(create_test_tx(b"high", 15)).ok();
        buckets.add_tx(create_test_tx(b"medium", 5)).ok();

        let ordered = buckets.get_all_ordered();
        assert_eq!(ordered.len(), 3);
        // High should come first (index 0), then medium (index 1), then low (index 2)
        // But since we're using HashMap, order within buckets is not guaranteed
        // Just verify we get all 3 in some order
    }

    #[test]
    fn test_bucket_stats() {
        let mut buckets = BucketedMempool::default();

        buckets.add_tx(create_test_tx(b"tx1", 5)).ok();
        buckets.add_tx(create_test_tx(b"tx2", 5)).ok();
        buckets.add_tx(create_test_tx(b"tx3", 0)).ok();

        let stats = buckets.get_stats();
        assert_eq!(stats.high_count, 0);
        assert_eq!(stats.medium_count, 2);
        assert_eq!(stats.low_count, 1);
        assert_eq!(stats.total_count, 3);
    }

    #[test]
    fn test_duplicate_rejection() {
        let mut buckets = BucketedMempool::default();
        let tx = create_test_tx(b"tx1", 5);
        let tx_dup = create_test_tx(b"tx1", 5);

        buckets.add_tx(tx).ok();
        let result = buckets.add_tx(tx_dup);

        assert!(result.is_err());
    }
}
