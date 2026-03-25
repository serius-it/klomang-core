use crate::core::dag::Dag;
use crate::core::crypto::Hash;
use crate::core::errors::CoreError;
use std::collections::HashSet;

/// Find common ancestor of two blocks
///
/// Returns: Some(ancestor_hash) if found, None if no common ancestor
pub fn find_common_ancestor(dag: &Dag, block_a: &Hash, block_b: &Hash) -> Result<Option<Hash>, CoreError> {
    if block_a == block_b {
        return Ok(Some(block_a.clone()));
    }

    // Collect all ancestors of block_a
    let mut ancestors_a = HashSet::new();
    let mut stack = vec![block_a.clone()];
    let mut visited = HashSet::new();

    while let Some(current) = stack.pop() {
        if visited.contains(&current) {
            continue;
        }
        visited.insert(current.clone());
        ancestors_a.insert(current.clone());

        if let Some(block) = dag.get_block(&current) {
            for parent in &block.parents {
                stack.push(parent.clone());
            }
        }
    }

    // Walk back from block_b until we find an ancestor in ancestors_a
    let mut stack = vec![block_b.clone()];
    visited.clear();

    while let Some(current) = stack.pop() {
        if visited.contains(&current) {
            continue;
        }
        visited.insert(current.clone());

        if ancestors_a.contains(&current) {
            return Ok(Some(current));
        }

        if let Some(block) = dag.get_block(&current) {
            for parent in &block.parents {
                stack.push(parent.clone());
            }
        }
    }

    Ok(None)
}