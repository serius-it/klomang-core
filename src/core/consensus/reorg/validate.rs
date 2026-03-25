use crate::core::consensus::reorg::snapshot::ReorgState;
use crate::core::errors::CoreError;

/// Validates reorganization parameters
pub fn validate_reorg(reorg: &ReorgState, max_reorg_depth: usize) -> Result<(), CoreError> {
    // Check if reorganization is needed
    if !reorg.is_needed() {
        return Err(CoreError::ConsensusError);
    }

    // Check reorganization depth
    if reorg.depth() > max_reorg_depth {
        return Err(CoreError::ConsensusError);
    }

    Ok(())
}