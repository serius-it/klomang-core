use crate::core::dag::Dag;
use crate::core::crypto::Hash;
use std::collections::HashSet;
use crate::core::errors::CoreError;

/// Calculate distance between two blocks (common ancestor path length)
pub fn calculate_path_length(dag: &Dag, from: &Hash, to: &Hash) -> Result<usize, CoreError> {
    if from == to {
        return Ok(0);
    }

    let mut visited = HashSet::new();
    let mut stack = vec![(from.clone(), 0)];

    while let Some((current, depth)) = stack.pop() {
        if current == *to {
            return Ok(depth);
        }

        if visited.contains(&current) {
            continue;
        }
        visited.insert(current.clone());

        if let Some(block) = dag.get_block(&current) {
            for parent in &block.parents {
                stack.push((parent.clone(), depth + 1));
            }
        }
    }

    Err(CoreError::ConsensusError)
}

/// Collect chain from ancestor to tip (in forward order) with determinism
///
/// Uses sorted HashSet iteration for deterministic path finding when multiple paths exist
pub fn collect_chain(dag: &Dag, current: &Hash, ancestor: &Hash) -> Result<Vec<Hash>, CoreError> {
    if current == ancestor {
        return Ok(vec![]);
    }

    // Use BFS with sorted stack to ensure deterministic traversal
    let mut came_from: std::collections::HashMap<Hash, Hash> = std::collections::HashMap::new();
    let mut bfs_queue = vec![current.clone()];
    let mut bfs_visited = std::collections::HashSet::new();
    let mut found = false;

    while let Some(node) = bfs_queue.pop() {
        if node == *ancestor {
            found = true;
            break;
        }

        if bfs_visited.contains(&node) {
            continue;
        }
        bfs_visited.insert(node.clone());

        if let Some(block) = dag.get_block(&node) {
            // Sort parents for deterministic traversal order
            let mut parents: Vec<_> = block.parents.iter().cloned().collect();
            parents.sort();

            for parent in parents {
                if !came_from.contains_key(&node) {
                    came_from.insert(node.clone(), parent.clone());
                }
                bfs_queue.push(parent);
            }
        }
    }

    if !found {
        return Err(CoreError::ConsensusError);
    }

    // Reconstruct path
    let mut chain = Vec::new();
    let mut current_node = current.clone();
    while current_node != *ancestor {
        chain.push(current_node.clone());
        if let Some(parent) = came_from.get(&current_node) {
            current_node = parent.clone();
        } else {
            // Should not happen if BFS found a path
            return Err(CoreError::ConsensusError);
        }
    }

    chain.reverse();
    Ok(chain)
}