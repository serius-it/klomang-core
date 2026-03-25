use crate::core::dag::{Dag, BlockNode};
use crate::core::consensus::GhostDag;
use crate::core::crypto::Hash;
use crate::core::errors::CoreError;
use super::path::collect_chain;
use super::ancestor::find_common_ancestor;
use super::snapshot::ReorgState;

/// Detect reorg need by comparing selected chains
///
/// Returns ReorgState if reorg is needed, None if current chain is better or equal
pub fn detect_reorg(
    dag: &Dag,
    ghostdag: &GhostDag,
    current_selected_tip: Option<&Hash>,
    new_block: &BlockNode,
) -> Result<Option<ReorgState>, CoreError> {
    let new_blue_score = new_block.blue_score;

    // Get current virtual block score
    let old_virtual = ghostdag.build_virtual_block(dag);
    let old_blue_score = old_virtual.blue_score;

    // If new block doesn't improve the score, no reorg needed
    if new_blue_score <= old_blue_score {
        return Ok(None);
    }

    // After adding new block, recompute and check
    // Find what would be the new selected chain
    let new_selected_tip = new_block.id.clone();

    // Find common ancestor
    let ancestor = if let Some(current_tip) = current_selected_tip {
        find_common_ancestor(dag, current_tip, &new_selected_tip)?
    } else {
        // No current tip, start from new block
        Some(new_selected_tip.clone())
    };

    match ancestor {
        None => {
            // No common ancestor found - should not happen in valid DAG
            return Err(CoreError::ConsensusError);
        }
        Some(common) => {
            if let Some(current_tip) = current_selected_tip {
                if current_tip == &new_selected_tip {
                    // Same tip, no reorg needed
                    return Ok(None);
                }

                // Collect blocks to disconnect
                let blocks_to_disconnect = collect_chain(dag, current_tip, &common)?;

                // Collect blocks to connect
                let blocks_to_connect = collect_chain(dag, &new_selected_tip, &common)?;

                // Only reorg if new chain is better
                if new_blue_score > old_blue_score {
                    let block_count_change = blocks_to_connect.len() as i32 - blocks_to_disconnect.len() as i32;

                    return Ok(Some(ReorgState {
                        blocks_to_disconnect,
                        blocks_to_connect,
                        common_ancestor: Some(common),
                        old_blue_score,
                        new_blue_score,
                        block_count_change,
                    }));
                }
            }

            Ok(None)
        }
    }
}

/// Check if reorg needed and return context for execution
///
/// Returns reorg state if needed, None if current chain is better
pub fn check_and_prepare_reorg(
    dag: &Dag,
    ghostdag: &GhostDag,
    current_selected_tip: Option<Hash>,
    new_block: &BlockNode,
) -> Result<Option<ReorgState>, CoreError> {
    detect_reorg(dag, ghostdag, current_selected_tip.as_ref(), new_block)
}