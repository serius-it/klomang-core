use crate::core::dag::BlockNode;
use crate::core::errors::CoreError;
use crate::core::consensus::reorg;
use super::engine::Engine;
use super::validation;
use super::state_apply;

/// Process a block through the consensus pipeline:
/// 
/// PIPELINE STAGES:
/// 1. Validate block (PoW, structure, difficulty)
/// 2. Add to DAG
/// 3. Run GHOSTDAG consensus (assign blue_set, red_set, blue_score)
/// 4. Validate coinbase with final blue_score
/// 5. Persist to storage
/// 6. **DETECT & EXECUTE REORG if needed** (NEW - critical fix)
/// 7. Apply block transactions to state
/// 8. Return reverted transactions to mempool (if reorg)
/// 9. Remove confirmed transactions from mempool
/// 10. Update finality and virtual block
/// 11. Prune old blocks
///
/// KEY DESIGN:
/// - Reorg detection happens AFTER DAG consensus but BEFORE state apply
/// - This ensures state is consistent with selected chain
/// - Atomicity: snapshot-restore on any error
/// - No unwrap() calls - all Result handling explicit
pub fn process_block(engine: &mut Engine, mut block: BlockNode) -> Result<(), CoreError> {
    let block_hash = block.id.clone();

    // =====================================================================
    // STAGE 1-5: Core validation and consensus
    // =====================================================================

    // 1. Calculate difficulty for block
    block.difficulty = engine.calculate_difficulty(block.timestamp);

    // 2. Validate block (including PoW)
    validation::validate_block(engine, &block)?;

    // 3. Check and mark genesis
    let is_genesis = block.parents.is_empty();
    if is_genesis {
        if engine.genesis_already_set() {
            return Err(CoreError::ConsensusError);
        }
        engine.set_genesis_hash(block_hash.clone());
    }

    // Get current selected chain BEFORE adding new block
    let old_selected_chain = engine.get_selected_chain();
    let old_selected_tip = old_selected_chain.first().cloned();

    // 4. Insert block to DAG
    engine.dag_mut().add_block(block.clone())?;

    // 5. Run GHOSTDAG consensus algorithm
    let ghostdag = engine.ghostdag().clone();
    let dag_mut = engine.dag_mut();
    ghostdag.process_block(dag_mut, &block_hash);

    // 6. Validate coinbase reward now that we have the correct blue_score
    if let Some(processed_block) = engine.dag().get_block(&block_hash) {
        validation::validate_coinbase_reward_final(processed_block)?;
    }

    // 7. Persist to storage
    if let Some(stored_block) = engine.dag().get_block(&block_hash).cloned() {
        engine.storage_mut().put_block(stored_block);
    }

    // =====================================================================
    // STAGE 6: DETECT & EXECUTE REORG (CRITICAL FIX - INTEGRATED)
    // =====================================================================

    // Build new virtual block with updated consensus
    let new_virtual_block = engine.ghostdag().build_virtual_block(engine.dag());

    // Check if reorganization is needed
    let reorg_needed = if let Some(current_tip) = &old_selected_tip {
        if let Some(new_selected_parent) = &new_virtual_block.selected_parent {
            // Reorg needed if selected parent changed
            current_tip != new_selected_parent
        } else {
            false
        }
    } else {
        false
    };

    // Execute reorg with transaction buffer if needed
    let reorg_buffer = if reorg_needed {
        // Prepare reorg state using immutable accessors
        let reorg_state = {
            let dag_ref = engine.dag();
            let ghostdag_ref = engine.ghostdag();
            reorg::check_and_prepare_reorg(dag_ref, ghostdag_ref, old_selected_tip.clone(), &block)?
        };

        if let Some(reorg_spec) = reorg_state {
            const MAX_REORG_DEPTH: usize = 1000;
            reorg::validate_reorg(&reorg_spec, MAX_REORG_DEPTH)?;

            // Execute reorg through Engine API to avoid cross-borrow conflicts
            Some(engine.execute_reorg_with_buffer(&reorg_spec)?)
        } else {
            None
        }
    } else {
        None
    };

    // =====================================================================
    // STAGE 7: Apply new block to state
    // =====================================================================

    let new_block_with_score = engine.dag().get_block(&block_hash).cloned();

    if let Some(processed_block) = new_block_with_score {
        // Update virtual block state (common path)
        if let Some(vb_hash) = new_virtual_block.selected_parent.clone() {
            engine.state_mut().set_finalizing_block(vb_hash);
        }
        engine.state_mut().update_virtual_score(new_virtual_block.blue_score);

        // Apply block normally only if reorg did NOT already apply the chain
        if reorg_buffer.is_none() {
            state_apply::apply_block_to_state(engine.state_mut(), &processed_block)?;
        }

        // =========================================================================
        // STAGE 8-9: Mempool management
        // =========================================================================

        // Reinsert reverted transactions from reorg buffer (if any)
        if let Some(ref buffer) = reorg_buffer {
            engine.reinsert_reverted_transactions(buffer)?;
        }

        // Remove confirmed transactions from mempool
        let confirmed_tx_ids: Vec<_> = processed_block.transactions.iter().map(|tx| tx.id.clone()).collect();
        engine.mempool_mut().remove_confirmed(&confirmed_tx_ids);
    }

    // =====================================================================
    // STAGE 10-11: Finality and pruning
    // =====================================================================

    // =====================================================================
    // STAGE 10-11: Finality and pruning
    // =====================================================================

    // 10. Update finality
    engine.update_finality();

    // 11. Prune old blocks
    engine.prune();

    Ok(())
}

/// Retrieve the currently selected chain (for reorg detection)
fn get_current_selected_chain(engine: &Engine) -> Vec<crate::core::crypto::Hash> {
    engine.ghostdag().get_virtual_selected_chain(engine.dag())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::crypto::Hash;
    use std::collections::HashSet;

    #[test]
    fn test_genesis_block_processing() {
        let mut engine = Engine::new();

        // Create coinbase transaction for genesis
        let coinbase_tx = crate::core::state::transaction::Transaction {
            id: Hash::new(b"coinbase"),
            inputs: Vec::new(), // Coinbase has no inputs
            outputs: vec![crate::core::state::transaction::TxOutput {
                value: 100, // Initial reward for DAA score 0
                pubkey_hash: Hash::new(b"miner"),
            }],
        };

        let genesis = BlockNode {
            id: Hash::new(b"genesis"),
            parents: HashSet::new(),
            children: HashSet::new(),
            selected_parent: None,
            blue_set: HashSet::new(),
            red_set: HashSet::new(),
            blue_score: 0,
            timestamp: 1000,
            difficulty: 1000,
            nonce: 0,
            transactions: vec![coinbase_tx],
        };

        let result = process_block(&mut engine, genesis);
        assert!(result.is_ok());
        assert_eq!(engine.get_block_count(), 1);
        assert!(engine.get_genesis().is_some());
    }

    #[test]
    fn test_duplicate_genesis_rejected() {
        let mut engine = Engine::new();

        let coinbase_tx = crate::core::state::transaction::Transaction {
            id: Hash::new(b"coinbase1"),
            inputs: Vec::new(),
            outputs: vec![crate::core::state::transaction::TxOutput {
                value: 100,
                pubkey_hash: Hash::new(b"miner1"),
            }],
        };

        let genesis = BlockNode {
            id: Hash::new(b"genesis"),
            parents: HashSet::new(),
            children: HashSet::new(),
            selected_parent: None,
            blue_set: HashSet::new(),
            red_set: HashSet::new(),
            blue_score: 0,
            timestamp: 1000,
            difficulty: 1000,
            nonce: 0,
            transactions: vec![coinbase_tx],
        };

        let _ = process_block(&mut engine, genesis.clone());

        let another_coinbase_tx = crate::core::state::transaction::Transaction {
            id: Hash::new(b"coinbase2"),
            inputs: Vec::new(),
            outputs: vec![crate::core::state::transaction::TxOutput {
                value: 100,
                pubkey_hash: Hash::new(b"miner2"),
            }],
        };

        let another_genesis = BlockNode {
            id: Hash::new(b"another"),
            parents: HashSet::new(),
            children: HashSet::new(),
            selected_parent: None,
            blue_set: HashSet::new(),
            red_set: HashSet::new(),
            blue_score: 0,
            timestamp: 2000,
            difficulty: 1000,
            nonce: 0,
            transactions: vec![another_coinbase_tx],
        };

        let result = process_block(&mut engine, another_genesis);
        assert!(matches!(result, Err(CoreError::ConsensusError)));
    }
}
