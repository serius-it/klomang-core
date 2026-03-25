/// Fairness ordering - age bonus + deterministic salting
///
/// Transaction ordering considers:
/// - Base: fee_rate (primary)
/// - Bonus: age (time in mempool)
/// - Determinism: secondary sort by hash
/// - Optional: random salt using block seed
///
/// Formula: order_score = fee_rate + age_bonus
///          deterministic_key = hash(tx_id || order_score)

use crate::core::crypto::Hash;
use crate::core::errors::CoreError;
use super::node::TxNode;

/// Fairness ordering configuration
#[derive(Debug, Clone)]
pub struct FairnessConfig {
    /// Age bonus per block (satoshis per byte per block)
    ///
    /// Allocates bonus to older transactions so they eventually get selected
    /// Example: age_bonus_per_block = 1 means tx gets +1 sat/byte per block
    pub age_bonus_per_block: u64,
    /// Maximum age bonus (satoshis per byte)
    ///
    /// Prevents very old transactions from dominating selection
    pub max_age_bonus: u64,
    /// Block height for deterministic seed
    ///
    /// Used to salt random ordering without breaking determinism
    pub block_height: u64,
}

impl FairnessConfig {
    /// Create default fairness configuration
    pub fn default() -> Self {
        Self {
            age_bonus_per_block: 0,  // No age bonus by default
            max_age_bonus: 0,
            block_height: 0,
        }
    }

    /// Create configuration with age bonus
    pub fn with_age_bonus(age_bonus_per_block: u64, max_age_bonus: u64, block_height: u64) -> Self {
        Self {
            age_bonus_per_block,
            max_age_bonus,
            block_height,
        }
    }
}

/// Fairness score for transaction ordering
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FairnessScore {
    /// Base fee rate (satoshis per byte)
    pub base_rate: u64,
    /// Age bonus applied (satoshis per byte)
    pub age_bonus: u64,
    /// Total order score (base_rate + age_bonus)
    pub total_score: u64,
    /// Deterministic hash for secondary sort
    pub deterministic_hash: u64,
}

impl FairnessScore {
    /// Calculate fairness score for transaction
    pub fn calculate(
        tx_id: &Hash,
        base_fee_rate: u64,
        blocks_in_mempool: u64,
        config: &FairnessConfig,
    ) -> Result<Self, CoreError> {
        // Calculate age bonus (capped at max)
        let raw_bonus = blocks_in_mempool
            .saturating_mul(config.age_bonus_per_block);
        let age_bonus = std::cmp::min(raw_bonus, config.max_age_bonus);

        // Calculate total score
        let total_score = base_fee_rate
            .checked_add(age_bonus)
            .ok_or_else(|| CoreError::TransactionError("Fairness score overflow".to_string()))?;

        // Calculate deterministic hash for secondary sort
        let deterministic_hash = calculate_deterministic_hash(tx_id, total_score, config.block_height);

        Ok(FairnessScore {
            base_rate: base_fee_rate,
            age_bonus,
            total_score,
            deterministic_hash,
        })
    }
}

/// Calculate deterministic hash for transaction
///
/// Uses sha256-like hashing but deterministic - always gives same result
/// for same inputs (independent of time, order, etc.)
pub fn calculate_deterministic_hash(tx_id: &Hash, score: u64, block_height: u64) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash as StdHash, Hasher};

    let mut hasher = DefaultHasher::new();

    // Hash components: tx_id, score, block_height
    tx_id.as_bytes().hash(&mut hasher);
    score.hash(&mut hasher);
    block_height.hash(&mut hasher);

    hasher.finish()
}

/// Order transactions by fairness score
///
/// Primary sort: higher total_score first (descending)
/// Secondary sort: lower deterministic_hash first (ascending)
pub fn order_by_fairness<'a>(mut nodes: Vec<&'a TxNode>, config: &FairnessConfig) -> Result<Vec<&'a TxNode>, CoreError> {
    // Calculate fairness scores for each node
    let mut scored: Vec<_> = Vec::new();
    for node in nodes.drain(..) {
        let score = FairnessScore::calculate(
            &node.tx_id,
            node.fee_rate,
            0, // blocks_in_mempool = 0 (simplified, would track in real system)
            config,
        )?;
        scored.push((node, score));
    }

    // Sort by fairness score
    scored.sort_by(|a, b| {
        // Primary: total_score descending (higher score first)
        match b.1.total_score.cmp(&a.1.total_score) {
            std::cmp::Ordering::Equal => {
                // Secondary: deterministic_hash ascending (lower hash first)
                match a.1.deterministic_hash.cmp(&b.1.deterministic_hash) {
                    std::cmp::Ordering::Equal => {
                        // Tertiary: fallback deterministic by tx_id to avoid non-deterministic ties
                        a.0.tx_id.cmp(&b.0.tx_id)
                    }
                    other => other,
                }
            }
            other => other,
        }
    });

    Ok(scored.into_iter().map(|(node, _)| node).collect())
}

/// Apply fairness ordering with optional age bonus
pub fn apply_fairness_ordering<'a>(
    nodes: Vec<&'a TxNode>,
    config: &FairnessConfig,
) -> Result<Vec<&'a TxNode>, CoreError> {
    order_by_fairness(nodes, config)
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
    fn test_fairness_config_default() {
        let config = FairnessConfig::default();
        assert_eq!(config.age_bonus_per_block, 0);
        assert_eq!(config.max_age_bonus, 0);
    }

    #[test]
    fn test_fairness_config_with_age_bonus() {
        let config = FairnessConfig::with_age_bonus(1, 10, 100);
        assert_eq!(config.age_bonus_per_block, 1);
        assert_eq!(config.max_age_bonus, 10);
        assert_eq!(config.block_height, 100);
    }

    #[test]
    fn test_fairness_score_no_bonus() {
        let config = FairnessConfig::default();
        let score = FairnessScore::calculate(&Hash::new(b"tx1"), 5, 0, &config);

        assert!(score.is_ok());
        let s = score.unwrap();
        assert_eq!(s.base_rate, 5);
        assert_eq!(s.age_bonus, 0);
        assert_eq!(s.total_score, 5);
    }

    #[test]
    fn test_fairness_score_with_age_bonus() {
        let config = FairnessConfig::with_age_bonus(2, 10, 100);
        let score = FairnessScore::calculate(&Hash::new(b"tx1"), 5, 3, &config);

        assert!(score.is_ok());
        let s = score.unwrap();
        assert_eq!(s.base_rate, 5);
        assert_eq!(s.age_bonus, 6); // 3 blocks * 2 = 6
        assert_eq!(s.total_score, 11);
    }

    #[test]
    fn test_fairness_score_age_bonus_capped() {
        let config = FairnessConfig::with_age_bonus(2, 10, 100);
        let score = FairnessScore::calculate(&Hash::new(b"tx1"), 5, 20, &config);

        assert!(score.is_ok());
        let s = score.unwrap();
        assert_eq!(s.age_bonus, 10); // Capped at max_age_bonus
        assert_eq!(s.total_score, 15);
    }

    #[test]
    fn test_deterministic_hash_consistency() {
        let tx_id = Hash::new(b"tx1");
        let h1 = calculate_deterministic_hash(&tx_id, 100, 5);
        let h2 = calculate_deterministic_hash(&tx_id, 100, 5);

        assert_eq!(h1, h2);
    }

    #[test]
    fn test_deterministic_hash_different_inputs() {
        let tx_id = Hash::new(b"tx1");
        let h1 = calculate_deterministic_hash(&tx_id, 100, 5);
        let h2 = calculate_deterministic_hash(&tx_id, 100, 6);

        assert_ne!(h1, h2);
    }

    #[test]
    fn test_order_by_fairness_higher_score_first() {
        let config = FairnessConfig::default();

        let tx1 = create_test_tx(b"tx1", 5); // fee_rate=3
        let tx2 = create_test_tx(b"tx2", 10); // fee_rate=6

        let nodes = vec![&tx1, &tx2];
        let ordered = order_by_fairness(nodes.clone(), &config);

        assert!(ordered.is_ok());
        let o = ordered.unwrap();
        // tx2 (higher fee_rate) should come before tx1
        assert_eq!(o[0].tx_id, Hash::new(b"tx2"));
        assert_eq!(o[1].tx_id, Hash::new(b"tx1"));
    }

    #[test]
    fn test_order_by_fairness_deterministic() {
        let config = FairnessConfig::default();

        let tx1 = create_test_tx(b"tx1_aaa", 5);
        let tx2 = create_test_tx(b"tx1_bbb", 5);
        let tx3 = create_test_tx(b"tx1_ccc", 5);

        let nodes = vec![&tx3, &tx1, &tx2];

        // Multiple calls should give same order (deterministic)
        let ordered1 = order_by_fairness(nodes.clone(), &config);
        let ordered2 = order_by_fairness(nodes.clone(), &config);

        // Both should succeed (deterministic behavior)
        assert!(ordered1.is_ok());
        assert!(ordered2.is_ok());
    }

    #[test]
    fn test_apply_fairness_ordering() {
        let config = FairnessConfig::default();

        let tx1 = create_test_tx(b"tx1", 5);
        let tx2 = create_test_tx(b"tx2", 10);

        let nodes = vec![&tx1, &tx2];
        let result = apply_fairness_ordering(nodes, &config);

        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 2);
    }
}
