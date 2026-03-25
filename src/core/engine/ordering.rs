use crate::core::crypto::Hash;
use crate::core::dag::Dag;
use crate::core::consensus::GhostDag;
use crate::core::consensus::ghostdag::VirtualBlock;
use std::collections::HashSet;

pub fn get_ordered_blocks(dag: &Dag, ghostdag: &GhostDag) -> Vec<Hash> {
    let virtual_block = match ghostdag.get_virtual_block(dag) {
        Some(vb) => vb,
        None => return vec![],
    };

    // Build selected parent chain (backbone)
    let mut chain = vec![];
    let mut current = Some(virtual_block.clone());
    while let Some(hash) = current {
        chain.push(hash.clone());
        current = dag.get_block(&hash).and_then(|b| b.selected_parent.clone());
    }
    chain.reverse(); // from genesis to virtual

    // Get blue set of virtual block
    let blue_set = ghostdag.get_blue_set(dag, &virtual_block);

    // Red set: all blocks not in chain or blue_set
    let all_blocks: HashSet<_> = dag.get_all_hashes().into_iter().collect();
    let chain_set: HashSet<_> = chain.iter().cloned().collect();
    let red_set: HashSet<_> = all_blocks.difference(&chain_set).filter(|h| !blue_set.contains(h)).cloned().collect();

    // Sort blue_set and red_set deterministically
    let mut blue_sorted: Vec<_> = blue_set.into_iter().collect();
    blue_sorted.sort_by(|a, b| {
        let a_score = dag.get_block(a).map_or(0, |b| b.blue_score);
        let b_score = dag.get_block(b).map_or(0, |b| b.blue_score);
        b_score.cmp(&a_score).then(a.cmp(b))
    });

    let mut red_sorted: Vec<_> = red_set.into_iter().collect();
    red_sorted.sort_by(|a, b| {
        let a_score = dag.get_block(a).map_or(0, |b| b.blue_score);
        let b_score = dag.get_block(b).map_or(0, |b| b.blue_score);
        b_score.cmp(&a_score).then(a.cmp(b))
    });

    // Combine: chain + blue_sorted + red_sorted
    let mut ordered = chain;
    ordered.extend(blue_sorted);
    ordered.extend(red_sorted);
    ordered
}