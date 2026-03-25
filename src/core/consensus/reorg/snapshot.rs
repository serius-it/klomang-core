use crate::core::crypto::Hash;

/// Reorganization state for atomic transactions
#[derive(Debug, Clone)]
pub struct ReorgState {
    /// Blocks to disconnect (in reverse order)
    pub blocks_to_disconnect: Vec<Hash>,
    /// Blocks to connect (in order)
    pub blocks_to_connect: Vec<Hash>,
    /// Common ancestor block
    pub common_ancestor: Option<Hash>,
    /// Previous blue_score
    pub old_blue_score: u64,
    /// New blue_score
    pub new_blue_score: u64,
    /// Block count delta
    pub block_count_change: i32,
}

impl ReorgState {
    /// Check if reorganization is needed
    pub fn is_needed(&self) -> bool {
        self.new_blue_score > self.old_blue_score
    }

    /// Get depth of reorganization (number of blocks changed)
    pub fn depth(&self) -> usize {
        self.blocks_to_disconnect.len() + self.blocks_to_connect.len()
    }
}