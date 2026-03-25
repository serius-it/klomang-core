use crate::core::dag::Dag;
use crate::core::state::BlockchainState;
use crate::core::crypto::Hash;
use crate::core::errors::CoreError;
use super::snapshot::ReorgState;

/// Apply blocks in forward order
pub fn apply_blocks(
    dag: &Dag,
    state: &mut BlockchainState,
    blocks: &[Hash],
) -> Result<(), CoreError> {
    for block_hash in blocks {
        if let Some(block) = dag.get_block(block_hash) {
            state.apply_block(block)?;
        } else {
            return Err(CoreError::TransactionError("Block not found during apply".to_string()));
        }
    }

    Ok(())
}

/// Collect reverted transactions from blocks (for mempool re-inclusion)
pub fn collect_reverted_transactions(
    dag: &Dag,
    blocks: &[Hash],
) -> Result<Vec<crate::core::state::transaction::Transaction>, CoreError> {
    let mut reverted_txs = Vec::new();

    // Collect in reverse order (just as we reverted)
    for block_hash in blocks.iter().rev() {
        if let Some(block) = dag.get_block(block_hash) {
            // Collect all non-coinbase transactions
            for tx in &block.transactions {
                if !tx.is_coinbase() {
                    reverted_txs.push(tx.clone());
                }
            }
        } else {
            return Err(CoreError::TransactionError("Block not found during tx collection".to_string()));
        }
    }

    Ok(reverted_txs)
}

/// Public wrapper for collecting reverted transactions from a reorg state
/// Used by engine to prefetch transactions before state modifications
pub fn collect_reverted_transactions_from_reorg(
    dag: &Dag,
    reorg: &ReorgState,
) -> Result<Vec<crate::core::state::transaction::Transaction>, CoreError> {
    collect_reverted_transactions(dag, &reorg.blocks_to_disconnect)
}