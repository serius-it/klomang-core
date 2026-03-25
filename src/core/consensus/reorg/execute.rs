use crate::core::dag::Dag;
use crate::core::consensus::GhostDag;
use crate::core::state::BlockchainState;
use crate::core::errors::CoreError;
use super::rollback::rollback_blocks;
use super::apply::apply_blocks;
use super::snapshot::ReorgState;
use super::ReorgTxBuffer;

/// Execute reorganization atomically
///
/// ATOMICITY: Ensures all-or-nothing state transition
pub fn execute_reorg(
    _ghostdag: &GhostDag,
    dag: &Dag,
    state: &mut BlockchainState,
    reorg: ReorgState,
) -> Result<(), CoreError> {
    if !reorg.is_needed() {
        return Ok(());
    }

    // Step 1: Create snapshot for rollback capability
    let snapshot = state.snapshot();

    // Step 2: Rollback old blocks
    if let Err(e) = rollback_blocks(dag, state, &reorg.blocks_to_disconnect) {
        // Restore snapshot on failure
        *state = snapshot;
        return Err(e);
    }

    // Step 3: Apply new blocks
    if let Err(e) = apply_blocks(dag, state, &reorg.blocks_to_connect) {
        // Restore snapshot on failure
        *state = snapshot;
        return Err(e);
    }

    // Success - state is now updated
    Ok(())
}

/// Execute reorganization with transaction buffer collection
///
/// Collects reverted transactions into buffer without touching mempool.
/// Returns buffer for safe reinsertion after state commit.
pub fn execute_reorg_with_buffer(
    dag: &Dag,
    state: &mut BlockchainState,
    reorg: &ReorgState,
) -> Result<ReorgTxBuffer, CoreError> {
    if !reorg.is_needed() {
        return Ok(ReorgTxBuffer::new());
    }

    // Collect reverted transactions BEFORE any state changes
    let reverted_txs = super::apply::collect_reverted_transactions_from_reorg(dag, reorg)?;

    // Execute reorg with recovery (atomic)
    execute_reorg_with_recovery(dag, state, reverted_txs, reorg)
}

/// Execute reorganization and return reverted transactions for mempool
///
/// This is the high-level interface that:
/// 1. Takes a snapshot of state BEFORE ANY CHANGES
/// 2. Rolls back old chain
/// 3. Applies new chain
/// 4. Returns reverted transactions for re-inclusion in mempool
/// 5. Restores snapshot on ANY failure (atomic)
///
/// Parameters:
/// - dag: Reference to the DAG (for block lookups)
/// - state: Mutable reference to state (will be modified)
/// - reverted_txs_prefetch: Pre-collected reverted transactions
/// - reorg: The reorg state specifying old and new chains
pub fn execute_reorg_with_recovery(
    dag: &Dag,
    state: &mut BlockchainState,
    reverted_txs_prefetch: Vec<crate::core::state::transaction::Transaction>,
    reorg: &ReorgState,
) -> Result<ReorgTxBuffer, CoreError> {
    if !reorg.is_needed() {
        return Ok(ReorgTxBuffer::new());
    }

    // Step 1: Create snapshot BEFORE ANY CHANGES
    let snapshot = state.snapshot();

    // Step 2: Rollback old blocks
    if let Err(e) = rollback_blocks(dag, state, &reorg.blocks_to_disconnect) {
        // Restore snapshot on failure
        state.restore(snapshot);
        return Err(e);
    }

    // Step 3: Apply new blocks
    if let Err(e) = apply_blocks(dag, state, &reorg.blocks_to_connect) {
        // Restore snapshot on failure
        state.restore(snapshot);
        return Err(e);
    }

    // Step 4: Success - return reverted txs for mempool re-inclusion
    Ok(ReorgTxBuffer::with_transactions(reverted_txs_prefetch))
}