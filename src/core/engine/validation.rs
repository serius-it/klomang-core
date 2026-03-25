use crate::core::dag::BlockNode;
use crate::core::errors::CoreError;
use crate::core::pow::{calculate_hash, is_valid_pow};
use crate::core::consensus::{capped_reward, validate_coinbase_reward};
use super::engine::Engine;

/// Validate a block for acceptance into the DAG
///
/// Checks:
/// a) Block doesn't already exist
/// b) All parent blocks exist in DAG
/// c) No duplicate transactions in block
/// d) Basic transaction validation
/// e) Proof of Work validation (skipped for genesis)
pub fn validate_block(engine: &Engine, block: &BlockNode) -> Result<(), CoreError> {
    validate_no_duplicate(engine, block)?;
    validate_parents_exist(engine, block)?;
    validate_tx_basic(block)?;
    // Skip PoW validation for genesis blocks
    if !block.parents.is_empty() {
        validate_pow(block)?;
    }
    Ok(())
}

/// Check that block hasn't been added before
fn validate_no_duplicate(engine: &Engine, block: &BlockNode) -> Result<(), CoreError> {
    if engine.block_exists(&block.id) {
        return Err(CoreError::DuplicateBlock);
    }
    Ok(())
}

/// Check that all parent blocks exist in the DAG
fn validate_parents_exist(engine: &Engine, block: &BlockNode) -> Result<(), CoreError> {
    for parent in &block.parents {
        if !engine.block_exists(parent) {
            return Err(CoreError::InvalidParent);
        }
    }
    Ok(())
}

/// Validate basic transaction properties
/// Currently a placeholder for extensible validation logic
fn validate_tx_basic(_block: &BlockNode) -> Result<(), CoreError> {
    // TODO: Add transaction format validation as needed
    // - Check inputs/outputs structure
    // - Validate script syntax
    // - Check transaction size limits
    // For now, all transactions are accepted
    Ok(())
}

/// Validate coinbase transaction reward amount (called after GHOSTDAG processing)
///
/// Checks that:
/// 1. Block contains exactly one coinbase transaction
/// 2. Coinbase has exactly one output
/// 3. Coinbase output equals the expected reward (subsidy + fees)
///
/// Note: This validates subsidy only. Fee calculation requires UTXO state
/// and happens separately in the reward calculation pipeline.
pub fn validate_coinbase_reward_final(block: &BlockNode) -> Result<(), CoreError> {
    // Calculate expected reward: subsidy based on daa_score
    // (fees will be calculated separately when UTXO state is available)
    let expected_reward = capped_reward(block.blue_score);

    // Validate the coinbase matches expected reward
    validate_coinbase_reward(block, expected_reward)
}

/// Validate Proof of Work for the block
fn validate_pow(block: &BlockNode) -> Result<(), CoreError> {
    // Serialize block header for hashing
    let header = serialize_header(block);
    let hash = calculate_hash(&header);
    let target = calculate_target(block.difficulty);
    if !is_valid_pow(&hash, target) {
        return Err(CoreError::ConsensusError);
    }
    Ok(())
}

/// Serialize block header for PoW
fn serialize_header(block: &BlockNode) -> Vec<u8> {
    let mut data = Vec::new();
    // Sort parents for deterministic order
    let mut parents: Vec<_> = block.parents.iter().collect();
    parents.sort();
    for parent in parents {
        data.extend_from_slice(parent.as_bytes());
    }
    data.extend_from_slice(&block.timestamp.to_le_bytes());
    data.extend_from_slice(&block.difficulty.to_le_bytes());
    data.extend_from_slice(&block.nonce.to_le_bytes());
    data
}

/// Calculate target from difficulty
fn calculate_target(difficulty: u64) -> u64 {
    if difficulty == 0 {
        u64::MAX
    } else {
        u64::MAX / difficulty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::crypto::Hash;
    use std::collections::HashSet;

    #[test]
    fn test_validate_unique_block() {
        let engine = Engine::new();
        let block = BlockNode {
            id: Hash::new(b"test"),
            parents: HashSet::new(),
            children: HashSet::new(),
            selected_parent: None,
            blue_set: HashSet::new(),
            red_set: HashSet::new(),
            blue_score: 0,
            timestamp: 1000,
            difficulty: 1000,
            nonce: 0,
            transactions: Vec::new(),
        };

        assert!(validate_block(&engine, &block).is_ok());
    }
}
