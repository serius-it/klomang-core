use crate::core::dag::Dag;
use crate::core::state::BlockchainState;
use crate::core::crypto::Hash;
use crate::core::errors::CoreError;

/// Rollback state by reverting block transactions in reverse order
pub fn rollback_blocks(
    dag: &Dag,
    state: &mut BlockchainState,
    blocks: &[Hash],
) -> Result<(), CoreError> {
    // Revert blocks in reverse order (last block's transactions first)
    for block_hash in blocks.iter().rev() {
        if let Some(block) = dag.get_block(block_hash) {
            state.revert_block(block)?;
        } else {
            return Err(CoreError::TransactionError("Block not found during rollback".to_string()));
        }
    }

    Ok(())
}