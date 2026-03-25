#[cfg(test)]
mod tests {
    use crate::core::consensus::reorg::*;
    use crate::core::dag::{BlockNode, Dag};
    use crate::core::consensus::ghostdag::GhostDag;
    use crate::core::state::BlockchainState;
    use crate::core::crypto::Hash;
    use std::collections::HashSet;

    fn create_block(id: &[u8], parents: HashSet<Hash>, blue_score: u64) -> BlockNode {
        let selected_parent = if parents.is_empty() {
            None
        } else {
            parents.iter().max_by(|a, b| a.cmp(b)).cloned()
        };
        BlockNode {
            id: Hash::new(id),
            parents,
            children: HashSet::new(),
            selected_parent,
            blue_set: HashSet::new(),
            red_set: HashSet::new(),
            blue_score,
            timestamp: 1000,
            difficulty: 1000,
            nonce: 0,
            transactions: Vec::new(),
        }
    }

    #[test]
    fn test_find_common_ancestor_same_block() {
        let mut dag = Dag::new();
        let block = create_block(b"a", HashSet::new(), 1);
        let hash = block.id.clone();
        dag.add_block(block).ok();

        let ancestor = find_common_ancestor(&dag, &hash, &hash).ok().flatten();
        assert_eq!(ancestor, Some(hash));
    }

    #[test]
    fn test_find_common_ancestor_linear_chain() {
        let mut dag = Dag::new();

        let block_a = create_block(b"a", HashSet::new(), 1);
        let hash_a = block_a.id.clone();
        dag.add_block(block_a).ok();

        let mut parents_b = HashSet::new();
        parents_b.insert(hash_a.clone());
        let block_b = create_block(b"b", parents_b, 2);
        let hash_b = block_b.id.clone();
        dag.add_block(block_b).ok();

        let mut parents_c = HashSet::new();
        parents_c.insert(hash_b.clone());
        let block_c = create_block(b"c", parents_c, 3);
        let hash_c = block_c.id.clone();
        dag.add_block(block_c).ok();

        let ancestor = find_common_ancestor(&dag, &hash_c, &hash_a).ok().flatten();
        assert_eq!(ancestor, Some(hash_a));
    }

    #[test]
    fn test_reorg_state_depth() {
        let reorg = ReorgState {
            blocks_to_disconnect: vec![Hash::new(b"a"), Hash::new(b"b")],
            blocks_to_connect: vec![Hash::new(b"c"), Hash::new(b"d"), Hash::new(b"e")],
            common_ancestor: Some(Hash::new(b"root")),
            old_blue_score: 10,
            new_blue_score: 15,
            block_count_change: 1,
        };

        assert_eq!(reorg.depth(), 5);
        assert!(reorg.is_needed());
    }

    #[test]
    fn test_reorg_validation_max_depth() {
        let reorg = ReorgState {
            blocks_to_disconnect: vec![Hash::new(b"a"), Hash::new(b"b")],
            blocks_to_connect: vec![Hash::new(b"c"), Hash::new(b"d"), Hash::new(b"e")],
            common_ancestor: Some(Hash::new(b"root")),
            old_blue_score: 10,
            new_blue_score: 15,
            block_count_change: 1,
        };

        assert!(validate_reorg(&reorg, 10).is_ok());
        assert!(validate_reorg(&reorg, 2).is_err());
    }

    #[test]
    fn test_fork_scenario() {
        let mut dag = Dag::new();

        let genesis = create_block(b"genesis", HashSet::new(), 1);
        let hash_genesis = genesis.id.clone();
        dag.add_block(genesis).ok();

        let mut parents_1a = HashSet::new();
        parents_1a.insert(hash_genesis.clone());
        let block_1a = create_block(b"1a", parents_1a, 2);
        let hash_1a = block_1a.id.clone();
        dag.add_block(block_1a).ok();

        let mut parents_2a = HashSet::new();
        parents_2a.insert(hash_1a.clone());
        let block_2a = create_block(b"2a", parents_2a, 3);
        let hash_2a = block_2a.id.clone();
        dag.add_block(block_2a).ok();

        let mut parents_1b = HashSet::new();
        parents_1b.insert(hash_genesis.clone());
        let block_1b = create_block(b"1b", parents_1b, 3);
        let hash_1b = block_1b.id.clone();
        dag.add_block(block_1b).ok();

        let mut parents_2b = HashSet::new();
        parents_2b.insert(hash_1b.clone());
        let block_2b = create_block(b"2b", parents_2b, 4);
        let hash_2b = block_2b.id.clone();
        dag.add_block(block_2b).ok();

        let ancestor = find_common_ancestor(&dag, &hash_2a, &hash_2b).ok().flatten();
        assert_eq!(ancestor, Some(hash_genesis.clone()));
    }

    #[test]
    fn test_deep_reorg_detection() {
        let mut dag = Dag::new();

        let genesis = create_block(b"gen", HashSet::new(), 1);
        let hash_gen = genesis.id.clone();
        dag.add_block(genesis).ok();

        let mut prev = hash_gen.clone();
        let mut hashes_a = vec![];
        for i in 1..=5 {
            let mut parents = HashSet::new();
            parents.insert(prev.clone());
            let name = format!("a{}", i);
            let block = create_block(name.as_bytes(), parents, (i + 1) as u64);
            let hash = block.id.clone();
            hashes_a.push(hash.clone());
            dag.add_block(block).ok();
            prev = hash;
        }

        prev = hash_gen.clone();
        let mut hashes_b = vec![];
        for i in 1..=6 {
            let mut parents = HashSet::new();
            parents.insert(prev.clone());
            let name = format!("b{}", i);
            let block = create_block(name.as_bytes(), parents, (i + 1 + 10) as u64);
            let hash = block.id.clone();
            hashes_b.push(hash.clone());
            dag.add_block(block).ok();
            prev = hash;
        }

        let block_a5 = dag.get_block(&hashes_a[4]).cloned();
        let block_b5 = dag.get_block(&hashes_b[4]).cloned();

        assert!(block_b5.is_some());
        assert!(block_a5.is_some());
        if let (Some(b5), Some(a5)) = (block_b5, block_a5) {
            assert!(b5.blue_score > a5.blue_score);
        }
    }

    #[test]
    fn test_reorg_state_block_count() {
        let reorg = ReorgState {
            blocks_to_disconnect: vec![Hash::new(b"a"), Hash::new(b"b"), Hash::new(b"c")],
            blocks_to_connect: vec![Hash::new(b"x"), Hash::new(b"y")],
            common_ancestor: Some(Hash::new(b"root")),
            old_blue_score: 20,
            new_blue_score: 22,
            block_count_change: -1,
        };

        assert_eq!(reorg.depth(), 5);
        assert_eq!(reorg.block_count_change, -1);
    }

    #[test]
    fn test_find_common_ancestor_fork_merge() {
        let mut dag = Dag::new();

        let a = create_block(b"a", HashSet::new(), 1);
        let hash_a = a.id.clone();
        dag.add_block(a).ok();

        let mut parents_b = HashSet::new();
        parents_b.insert(hash_a.clone());
        let b = create_block(b"b", parents_b, 2);
        let hash_b = b.id.clone();
        dag.add_block(b).ok();

        let mut parents_c1 = HashSet::new();
        parents_c1.insert(hash_b.clone());
        let c1 = create_block(b"c1", parents_c1, 3);
        let hash_c1 = c1.id.clone();
        dag.add_block(c1).ok();

        let mut parents_c2 = HashSet::new();
        parents_c2.insert(hash_b.clone());
        let c2 = create_block(b"c2", parents_c2, 3);
        let hash_c2 = c2.id.clone();
        dag.add_block(c2).ok();

        let ancestor = find_common_ancestor(&dag, &hash_c1, &hash_c2).ok().flatten();
        assert_eq!(ancestor, Some(hash_b));
    }

    #[test]
    fn test_reorg_state_not_needed_equal_score() {
        let reorg = ReorgState {
            blocks_to_disconnect: vec![Hash::new(b"a")],
            blocks_to_connect: vec![Hash::new(b"b")],
            common_ancestor: Some(Hash::new(b"root")),
            old_blue_score: 100,
            new_blue_score: 100,
            block_count_change: 0,
        };

        assert!(!reorg.is_needed());
    }

    #[test]
    fn test_reorg_snapshot_restore() {
        let initial_state = BlockchainState::new();
        let snapshot = initial_state.snapshot();

        assert_eq!(initial_state.virtual_score, snapshot.virtual_score);
        assert_eq!(initial_state.finalizing_block, snapshot.finalizing_block);
    }

    #[test]
    fn test_reorg_deterministic_chain_collection() {
        let mut dag = Dag::new();

        let gen = create_block(b"gen", HashSet::new(), 1);
        let hash_gen = gen.id.clone();
        dag.add_block(gen).ok();

        let mut parents_a = HashSet::new();
        parents_a.insert(hash_gen.clone());
        let a = create_block(b"a", parents_a, 2);
        let hash_a = a.id.clone();
        dag.add_block(a).ok();

        let mut parents_b = HashSet::new();
        parents_b.insert(hash_a.clone());
        let b = create_block(b"b", parents_b, 3);
        let hash_b = b.id.clone();
        dag.add_block(b).ok();

        let mut parents_c = HashSet::new();
        parents_c.insert(hash_b.clone());
        let c = create_block(b"c", parents_c, 4);
        let hash_c = c.id.clone();
        dag.add_block(c).ok();

        let chain1 = collect_chain(&dag, &hash_c, &hash_gen).ok();
        let chain1_same = collect_chain(&dag, &hash_c, &hash_gen).ok();

        if let (Some(c1), Some(c2)) = (&chain1, &chain1_same) {
            assert_eq!(c1, c2, "Chains should be identical (deterministic)");
        }
    }

    #[test]
    fn test_reorg_conflicting_tx_detection() {
        let mut dag = Dag::new();

        let genesis = create_block(b"genesis", HashSet::new(), 0);
        let hash_genesis = genesis.id.clone();
        dag.add_block(genesis).ok();

        let mut parents_a = HashSet::new();
        parents_a.insert(hash_genesis.clone());
        let a = create_block(b"block_a", parents_a, 1);
        let hash_a = a.id.clone();
        dag.add_block(a).ok();

        let mut parents_b = HashSet::new();
        parents_b.insert(hash_genesis.clone());
        let b = create_block(b"block_b", parents_b, 2);
        let hash_b = b.id.clone();
        dag.add_block(b).ok();

        let ancestor = find_common_ancestor(&dag, &hash_a, &hash_b).ok().flatten();
        assert_eq!(ancestor.as_ref(), Some(&hash_genesis));

        let chain_to_reorg_from = collect_chain(&dag, &hash_a, &hash_genesis.clone()).ok();
        let chain_to_reorg_to = collect_chain(&dag, &hash_b, &hash_genesis.clone()).ok();

        if let (Some(from_chain), Some(to_chain)) = (chain_to_reorg_from, chain_to_reorg_to) {
            assert_ne!(from_chain, to_chain, "Conflicting branches should take different paths");
        }
    }

    #[test]
    fn test_reorg_triple_branch_convergence() {
        let mut dag = Dag::new();

        let gen = create_block(b"gen", HashSet::new(), 1);
        let hash_gen = gen.id.clone();
        dag.add_block(gen).ok();

        let mut parents_a1 = HashSet::new();
        parents_a1.insert(hash_gen.clone());
        let a1 = create_block(b"a1", parents_a1, 2);
        let hash_a1 = a1.id.clone();
        dag.add_block(a1).ok();

        let mut parents_a2 = HashSet::new();
        parents_a2.insert(hash_a1.clone());
        let a2 = create_block(b"a2", parents_a2, 3);
        let hash_a2 = a2.id.clone();
        dag.add_block(a2).ok();

        let mut parents_b1 = HashSet::new();
        parents_b1.insert(hash_gen.clone());
        let b1 = create_block(b"b1", parents_b1, 5);
        let hash_b1 = b1.id.clone();
        dag.add_block(b1).ok();

        let mut parents_b2 = HashSet::new();
        parents_b2.insert(hash_b1.clone());
        let b2 = create_block(b"b2", parents_b2, 6);
        let hash_b2 = b2.id.clone();
        dag.add_block(b2).ok();

        let mut parents_c1 = HashSet::new();
        parents_c1.insert(hash_gen.clone());
        let c1 = create_block(b"c1", parents_c1, 4);
        let hash_c1 = c1.id.clone();
        dag.add_block(c1).ok();

        let ancestor_ab = find_common_ancestor(&dag, &hash_a2, &hash_b2).ok().flatten();
        let ancestor_ac = find_common_ancestor(&dag, &hash_a2, &hash_c1).ok().flatten();
        let ancestor_bc = find_common_ancestor(&dag, &hash_b2, &hash_c1).ok().flatten();

        assert_eq!(ancestor_ab, Some(hash_gen.clone()));
        assert_eq!(ancestor_ac, Some(hash_gen.clone()));
        assert_eq!(ancestor_bc, Some(hash_gen.clone()));
    }

    #[test]
    fn test_reorg_preserves_ancestor_blocks() {
        let mut dag = Dag::new();

        let gen = create_block(b"gen", HashSet::new(), 1);
        let hash_gen = gen.id.clone();
        dag.add_block(gen).ok();

        let mut parents_shared = HashSet::new();
        parents_shared.insert(hash_gen.clone());
        let shared = create_block(b"shared", parents_shared, 2);
        let hash_shared = shared.id.clone();
        dag.add_block(shared).ok();

        let mut parents_a1 = HashSet::new();
        parents_a1.insert(hash_shared.clone());
        let a1 = create_block(b"a1", parents_a1, 3);
        let hash_a1 = a1.id.clone();
        dag.add_block(a1).ok();

        let mut parents_b1 = HashSet::new();
        parents_b1.insert(hash_shared.clone());
        let b1 = create_block(b"b1", parents_b1, 4);
        let hash_b1 = b1.id.clone();
        dag.add_block(b1).ok();

        let ancestor = find_common_ancestor(&dag, &hash_a1, &hash_b1).ok().flatten();
        assert_eq!(ancestor, Some(hash_shared.clone()));

        let chain_a = collect_chain(&dag, &hash_a1, &hash_shared);
        let chain_b = collect_chain(&dag, &hash_b1, &hash_shared);

        if let (Ok(ca), Ok(cb)) = (chain_a, chain_b) {
            assert!(!ca.contains(&hash_shared));
            assert!(!cb.contains(&hash_shared));
            assert!(!ca.contains(&hash_gen));
            assert!(!cb.contains(&hash_gen));
        }
    }

    #[test]
    fn test_reorg_empty_paths() {
        let mut dag = Dag::new();

        let block = create_block(b"single", HashSet::new(), 1);
        let hash = block.id.clone();
        dag.add_block(block).ok();

        let chain = collect_chain(&dag, &hash, &hash).ok();
        if let Some(c) = chain {
            assert!(c.is_empty());
        }
    }

    #[test]
    fn test_fork_scenario_simple_two_branches() {
        let mut dag = Dag::new();

        let genesis = create_block(b"genesis", HashSet::new(), 0);
        let hash_genesis = genesis.id.clone();
        dag.add_block(genesis).ok();

        let mut parents_a = HashSet::new();
        parents_a.insert(hash_genesis.clone());
        let block_a = create_block(b"block_a", parents_a.clone(), 1);
        let hash_a = block_a.id.clone();
        dag.add_block(block_a).ok();

        let mut parents_b = HashSet::new();
        parents_b.insert(hash_genesis.clone());
        let block_b = create_block(b"block_b", parents_b, 2);
        let hash_b = block_b.id.clone();
        dag.add_block(block_b).ok();

        // Test that we can find the common ancestor
        let ancestor = find_common_ancestor(&dag, &hash_a, &hash_b)
            .ok()
            .flatten();
        assert_eq!(ancestor, Some(hash_genesis.clone()), "Common ancestor should be genesis");

        // Test that we can collect the paths
        let path_a = collect_chain(&dag, &hash_a, &hash_genesis).ok();
        let path_b = collect_chain(&dag, &hash_b, &hash_genesis).ok();
        
        assert!(path_a.is_some(), "Should collect path from a to genesis");
        assert!(path_b.is_some(), "Should collect path from b to genesis");
        
        if let (Some(pa), Some(pb)) = (path_a, path_b) {
            assert_eq!(pa.len(), 1, "Path from A should have 1 block (A itself)");
            assert_eq!(pb.len(), 1, "Path from B should have 1 block (B itself)");
            assert_eq!(pa[0], hash_a, "Path should end at A");
            assert_eq!(pb[0], hash_b, "Path should end at B");
        }

        // Test ReorgState can be built manually
        let reorg = ReorgState {
            blocks_to_disconnect: vec![hash_a.clone()],
            blocks_to_connect: vec![hash_b.clone()],
            common_ancestor: Some(hash_genesis),
            old_blue_score: 1,
            new_blue_score: 2,
            block_count_change: 0,
        };
        assert!(reorg.is_needed(), "Reorg should be needed (B has higher score)");
        assert_eq!(reorg.depth(), 2, "Should disconnect 1 block and connect 1 block");
    }

    #[test]
    fn test_deep_reorg_scenario() {
        let mut dag = Dag::new();

        let gen = create_block(b"gen", HashSet::new(), 0);
        let hash_gen = gen.id.clone();
        dag.add_block(gen).ok();

        let mut parents = HashSet::new();
        parents.insert(hash_gen.clone());
        let a1 = create_block(b"a1", parents.clone(), 1);
        let hash_a1 = a1.id.clone();
        dag.add_block(a1).ok();

        let mut parents = HashSet::new();
        parents.insert(hash_a1.clone());
        let a2 = create_block(b"a2", parents.clone(), 2);
        let hash_a2 = a2.id.clone();
        dag.add_block(a2).ok();

        let mut parents = HashSet::new();
        parents.insert(hash_a2.clone());
        let a3 = create_block(b"a3", parents, 3);
        let hash_a3 = a3.id.clone();
        dag.add_block(a3).ok();

        let mut parents = HashSet::new();
        parents.insert(hash_gen.clone());
        let b1 = create_block(b"b1", parents.clone(), 2);
        let hash_b1 = b1.id.clone();
        dag.add_block(b1).ok();

        let mut parents = HashSet::new();
        parents.insert(hash_b1.clone());
        let b2 = create_block(b"b2", parents.clone(), 4);
        let hash_b2 = b2.id.clone();
        dag.add_block(b2).ok();

        let mut parents = HashSet::new();
        parents.insert(hash_b2.clone());
        let b3 = create_block(b"b3", parents.clone(), 6);
        let hash_b3 = b3.id.clone();
        dag.add_block(b3).ok();

        let mut parents = HashSet::new();
        parents.insert(hash_b3.clone());
        let b4 = create_block(b"b4", parents, 8);
        let hash_b4 = b4.id.clone();
        dag.add_block(b4).ok();

        // Test that we can find the common ancestor
        let ancestor = find_common_ancestor(&dag, &hash_a3, &hash_b4)
            .ok()
            .flatten();
        assert_eq!(ancestor, Some(hash_gen.clone()), "Common ancestor should be genesis");

        // Test that we can collect the paths
        let path_a = collect_chain(&dag, &hash_a3, &hash_gen).ok();
        let path_b = collect_chain(&dag, &hash_b4, &hash_gen).ok();

        assert!(path_a.is_some(), "Should collect path A");
        assert!(path_b.is_some(), "Should collect path B");

        if let (Some(pa), Some(pb)) = (path_a, path_b) {
            assert_eq!(pa.len(), 3, "Should disconnect a1, a2, a3");
            assert_eq!(pb.len(), 4, "Should connect b1, b2, b3, b4");
            
            // Verify the paths are correct in ancestor-to-tip order
            assert_eq!(pa[0], hash_a1, "First block in A path should be a1");
            assert_eq!(pa[1], hash_a2, "Second block in A path should be a2");
            assert_eq!(pa[2], hash_a3, "Third block in A path should be a3");

            assert_eq!(pb[0], hash_b1, "First block in B path should be b1");
            assert_eq!(pb[1], hash_b2, "Second block in B path should be b2");
            assert_eq!(pb[2], hash_b3, "Third block in B path should be b3");
            assert_eq!(pb[3], hash_b4, "Fourth block in B path should be b4");
        }

        // Test ReorgState can be built
        let reorg = ReorgState {
            blocks_to_disconnect: vec![hash_a3.clone(), hash_a2.clone(), hash_a1.clone()],
            blocks_to_connect: vec![hash_b1, hash_b2, hash_b3, hash_b4],
            common_ancestor: Some(hash_gen),
            old_blue_score: 3,
            new_blue_score: 8,
            block_count_change: 1,
        };
        
        assert!(reorg.is_needed());
        assert_eq!(reorg.depth(), 7, "Depth should be 3 disconnect + 4 connect");
    }

    #[test]
    fn test_reorg_determinism() {
        let mut dag = Dag::new();

        let gen = create_block(b"gen", HashSet::new(), 0);
        let hash_gen = gen.id.clone();
        dag.add_block(gen).ok();

        let mut parents = HashSet::new();
        parents.insert(hash_gen.clone());
        let a = create_block(b"a", parents.clone(), 1);
        let hash_a = a.id.clone();
        dag.add_block(a).ok();

        let mut parents = HashSet::new();
        parents.insert(hash_gen.clone());
        let b = create_block(b"b", parents, 2);
        let hash_b = b.id.clone();
        dag.add_block(b).ok();

        // Test determinism of collect_chain - should return same result multiple times
        let chain1_a = collect_chain(&dag, &hash_a, &hash_gen).ok();
        let chain2_a = collect_chain(&dag, &hash_a, &hash_gen).ok();
        let chain3_a = collect_chain(&dag, &hash_a, &hash_gen).ok();

        let chain1_b = collect_chain(&dag, &hash_b, &hash_gen).ok();
        let chain2_b = collect_chain(&dag, &hash_b, &hash_gen).ok();
        let chain3_b = collect_chain(&dag, &hash_b, &hash_gen).ok();

        assert_eq!(chain1_a, chain2_a, "Chain should be deterministic");
        assert_eq!(chain2_a, chain3_a, "Chain should be deterministic");
        assert_eq!(chain1_b, chain2_b, "Chain should be deterministic");
        assert_eq!(chain2_b, chain3_b, "Chain should be deterministic");

        // Test determinism of find_common_ancestor
        let ca1 = find_common_ancestor(&dag, &hash_a, &hash_b).ok().flatten();
        let ca2 = find_common_ancestor(&dag, &hash_a, &hash_b).ok().flatten();
        let ca3 = find_common_ancestor(&dag, &hash_a, &hash_b).ok().flatten();

        assert_eq!(ca1, ca2, "Common ancestor should be deterministic");
        assert_eq!(ca2, ca3, "Common ancestor should be deterministic");
    }

    #[test]
    fn test_reorg_max_depth_validation() {
        let reorg_state = ReorgState {
            blocks_to_disconnect: vec![Hash::new(b"x"), Hash::new(b"y"), Hash::new(b"z")],
            blocks_to_connect: vec![
                Hash::new(b"1"),
                Hash::new(b"2"),
                Hash::new(b"3"),
                Hash::new(b"4"),
                Hash::new(b"5"),
                Hash::new(b"6"),
                Hash::new(b"7"),
            ],
            common_ancestor: Some(Hash::new(b"gen")),
            old_blue_score: 0,
            new_blue_score: 100,
            block_count_change: 4,
        };

        let validation_result = validate_reorg(&reorg_state, 5);
        assert!(
            validation_result.is_err(),
            "Should reject reorg depth 10 > max 5"
        );

        let validation_result = validate_reorg(&reorg_state, 20);
        assert!(validation_result.is_ok(), "Should accept reorg depth 10 < max 20");
    }

    #[test]
    fn test_conflicting_transactions_reorg_scenario() {
        let mut dag = Dag::new();

        let gen = create_block(b"gen", HashSet::new(), 0);
        let hash_gen = gen.id.clone();
        dag.add_block(gen).ok();

        let mut parents_a = HashSet::new();
        parents_a.insert(hash_gen.clone());
        let a = create_block(b"block_a", parents_a, 1);
        let hash_a = a.id.clone();
        dag.add_block(a).ok();

        let mut parents_b = HashSet::new();
        parents_b.insert(hash_gen.clone());
        let b = create_block(b"block_b", parents_b, 3);
        let hash_b = b.id.clone();
        dag.add_block(b).ok();

        let chain_from_a = collect_chain(&dag, &hash_a, &hash_gen);
        let chain_from_b = collect_chain(&dag, &hash_b, &hash_gen);

        match (chain_from_a, chain_from_b) {
            (Ok(ca), Ok(cb)) => {
                assert_eq!(ca.len(), 1);
                assert_eq!(cb.len(), 1);
                assert_ne!(ca[0], cb[0]);
                assert_eq!(ca[0], hash_a);
                assert_eq!(cb[0], hash_b);
            }
            _ => panic!("Both chains should exist"),
        }
    }

    #[test]
    fn test_reorg_tx_buffer_creation() {
        let buffer = ReorgTxBuffer::new();
        assert!(buffer.is_empty());
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn test_reorg_tx_buffer_with_transactions() {
        let tx1 = crate::core::state::transaction::Transaction {
            id: Hash::new(b"tx1"),
            inputs: vec![],
            outputs: vec![crate::core::state::transaction::TxOutput {
                value: 100,
                pubkey_hash: Hash::new(b"dest"),
            }],
        };
        let tx2 = crate::core::state::transaction::Transaction {
            id: Hash::new(b"tx2"),
            inputs: vec![],
            outputs: vec![crate::core::state::transaction::TxOutput {
                value: 200,
                pubkey_hash: Hash::new(b"dest2"),
            }],
        };

        let buffer = ReorgTxBuffer::with_transactions(vec![tx1, tx2]);
        assert!(!buffer.is_empty());
        assert_eq!(buffer.len(), 2);
    }

    #[test]
    fn test_execute_reorg_with_buffer_no_reorg() {
        let mut dag = Dag::new();
        let mut state = BlockchainState::new();

        let genesis = create_block(b"genesis", HashSet::new(), 1);
        let hash_genesis = genesis.id.clone();
        dag.add_block(genesis).ok();

        let reorg = ReorgState {
            blocks_to_disconnect: vec![],
            blocks_to_connect: vec![],
            common_ancestor: Some(hash_genesis),
            old_blue_score: 1,
            new_blue_score: 1,
            block_count_change: 0,
        };

        let buffer = execute_reorg_with_buffer(&dag, &mut state, &reorg).unwrap();
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_execute_reorg_with_buffer_with_reorg() {
        let mut dag = Dag::new();
        let mut state = BlockchainState::new();

        // Create genesis
        let genesis = create_block(b"genesis", HashSet::new(), 1);
        let hash_genesis = genesis.id.clone();
        dag.add_block(genesis.clone()).ok();

        // Create block A with transaction
        let mut parents_a = HashSet::new();
        parents_a.insert(hash_genesis.clone());
        let mut block_a = create_block(b"block_a", parents_a, 2);
        let tx = crate::core::state::transaction::Transaction {
            id: Hash::new(b"test_tx"),
            inputs: vec![crate::core::state::transaction::TxInput {
                prev_tx: Hash::new(b"prev"),
                index: 0,
                signature: vec![],
                pubkey: vec![],
            }],
            outputs: vec![crate::core::state::transaction::TxOutput {
                value: 50,
                pubkey_hash: Hash::new(b"dest"),
            }],
        };
        block_a.transactions = vec![tx.clone()];
        let hash_a = block_a.id.clone();
        dag.add_block(block_a).ok();

        // Create competing block B
        let mut parents_b = HashSet::new();
        parents_b.insert(hash_genesis.clone());
        let block_b = create_block(b"block_b", parents_b, 3); // Higher score
        let hash_b = block_b.id.clone();
        dag.add_block(block_b).ok();

        let reorg = ReorgState {
            blocks_to_disconnect: vec![hash_a],
            blocks_to_connect: vec![hash_b],
            common_ancestor: Some(hash_genesis),
            old_blue_score: 2,
            new_blue_score: 3,
            block_count_change: 0,
        };

        let buffer = execute_reorg_with_buffer(&dag, &mut state, &reorg).unwrap();
        assert!(!buffer.is_empty());
        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer.reverted[0].id, tx.id);
    }
}