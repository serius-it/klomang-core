# Klomang-Core Codebase Analysis

## 1. REORG IMPLEMENTATION (`src/core/engine/reorg.rs`)

### Key Functions
| Function | Purpose | Status |
|----------|---------|--------|
| `detect_reorg()` | Compares selected chains by blue_score; returns ReorgState if new chain is better | ✅ Implemented |
| `find_common_ancestor()` | Finds LCA of two blocks using HashSet traversal | ✅ Implemented |
| `collect_chain()` | BFS path reconstruction from block to ancestor (deterministic via sorting) | ✅ Implemented |
| `execute_reorg()` | Atomic reorg with snapshot-based rollback | ✅ Implemented |
| `rollback_blocks()` | Reverts transactions in reverse order | ✅ Implemented |
| `apply_blocks()` | Applies blocks forward in order | ✅ Implemented |
| `validate_reorg()` | Prevents unreasonably deep reorgs (max_depth check) | ✅ Implemented |

### Current Capabilities
✅ **Strong:**
- **Atomicity**: Full snapshot-restore mechanism for all-or-nothing state transitions
- **Determinism**: Sorted traversal ensures reproducible path finding even with DAG fan-out
- **Detection**: Compares virtual block blue_scores to identify better chains
- **Depth Control**: Validates reorg depth against max_depth threshold
- **Path Finding**: Uses BFS with came_from map for reliable chain reconstruction

### Gaps for Full Integration
❌ **Missing:**
1. **Caller Integration**: `block_pipeline.rs` has TODO comment: "Full reorg integration requires refactoring to handle borrow checker constraints"
   - Reorg detection is computed but not executed during block processing
   - Reorg only runs if manually called (currently not invoked in pipeline)
   
2. **Borrow Checker Issues**:
   - Cannot hold mutable refs to (dag, state, ghostdag) simultaneously needed for execute_reorg
   - block_pipeline tries multiple borrows: `engine.dag()`, `engine.state_mut()`, `engine.ghostdag()`
   - Engine wrapper needed to enable reorg execution within pipeline

3. **Virtual Block Updates**:
   - After reorg, virtual block and selected chain not updated
   - New virtual block computed but not stored/returned for consensus

4. **Mempool Sync**:
   - Reorg doesn't handle mempool transaction invalidation
   - Reverted transactions not returned for re-entry

---

## 2. STATE MANAGEMENT (`src/core/engine/engine.rs` + `src/core/state/mod.rs`)

### Key Structures
```
Engine {
  dag: Dag,
  ghostdag: GhostDag,
  state: BlockchainState,
  mempool: Mempool,
  finalized_tip: Option<Hash>,
  storage: Box<dyn Storage>,
  genesis_hash: Option<Hash>,
}

BlockchainState {
  finalizing_block: Option<Hash>,
  virtual_score: u64,
  pruned: Vec<Hash>,
  utxo_set: UtxoSet,
}
```

### Key Engine Methods
| Method | Purpose | Scope |
|--------|---------|-------|
| `get_state()` | Immutable state access | Read-only |
| `get_virtual_block()` | Computes virtual block from GHOSTDAG | Read-only |
| `get_selected_chain()` | GHOSTDAG selected chain tip | Read-only |
| `get_finalized_tip()` | Returns block beyond finality_depth | Read-only |
| `update_finality()` | Sets finalized_tip at depth threshold | Mutating |
| `prune()` | Removes non-finalized old blocks | Mutating |
| `add_block()` | Full pipeline processing | Mutating |

### State Tracking
✅ **Implemented:**
- Finalizing block tracked for ordering
- Virtual score updated per block
- Pruned block list maintained
- Finality depth applied (2× finality_depth recent window preserved)

❌ **Limited:**
- No state history/snapshots at Engine level (only within BlockchainState.snapshot())
- No transaction-level state tracking (applied but not stored per block)
- No rollback path from Engine (only works within execute_reorg)
- No ancestor-state lookups (can't query state at arbitrary historical block)

---

## 3. UTXO IMPLEMENTATION (`src/core/state/utxo.rs`)

### Key Structure
```
UtxoSet {
  utxos: HashMap<(TxHash, OutputIndex), TxOutput>,
}
```

### Methods
| Method | Purpose | Capability |
|--------|---------|-----------|
| `validate_tx()` | Checks inputs exist, not double-spent | ✅ Forward-facing |
| `apply_tx()` | Removes spent inputs, adds new outputs | ✅ Forward-facing |
| `get_balance()` | Sum of outputs for pubkey_hash | ✅ Query |
| `new()` | Creates empty UTXO set | ✅ Init |

### Rollback Capability Analysis
⚠️ **Partial:**

**What Works:**
- `BlockchainState.snapshot()` clones entire HashMap → full rollback possible
- `execute_reorg()` uses snapshot restore for atomic restoration
- Snapshot-based rollback is production-sound (used in reorg)

**What Doesn't Work:**
- **No incremental revert**: `revert_block()` in BlockchainState has limitations:
  - Removes outputs added by tx (✅ works)
  - Cannot restore inputs because original output value not stored
  - Comment in code: "Since we use snapshot() cloning, full rollback works! This revert_block is supplementary"
  
**Workaround in Place:**
- Full snapshot cloning used in `execute_reorg()` → bypasses incremental revert limitation
- Atomic rollback works but memory-intensive for deep reorgs

### Gaps
❌ **Missing:**
1. No transaction-level UTXO deltas (can't reconstruct state at any block)
2. No spent output storage (cannot incremental rollback without full snapshot)
3. No height-indexed UTXO snapshots (state only at current tip)
4. No mempool UTXO tracking (no provisional state for pending txs)

---

## 4. BLOCK PIPELINE (`src/core/engine/block_pipeline.rs`)

### Pipeline Stages
```
process_block():
  1. Calculate difficulty (DAA)
  2. Validate block (PoW, coinbase)
  3. Mark/check genesis
  4. Get current selected chain (for reorg detection)
  5. Add to DAG
  6. Run GHOSTDAG consensus
  7. Validate coinbase reward (with correct blue_score)
  8. Persist to storage
  9. Build virtual block
 10. Apply transactions to state
 11. Remove confirmed txs from mempool
 12. Update finality
 13. Prune old blocks
```

### Current Implementation
✅ **Working:**
- Tax & fee calculation before DAG insertion
- GHOSTDAG consensus applied per-block
- Final coinbase validation with blue_score
- Transaction state applied to UTXO set
- Mempool cleanup of confirmed transactions

⚠️ **Incomplete:**
- **Reorg Integration**: Lines 55-60 note that reorg execution blocked by borrow checker
  - `detect_reorg()` never called
  - `execute_reorg()` never invoked
  - Virtual block + selected chain not reconciled before state apply
  - TODO comment suggests refactoring needed

### Gaps
❌ **Missing:**
1. **Reorg Execution**: detect_reorg() computed but not executed
2. **Virtual Block Selection**: Virtual block built but not used for state apply
3. **Transaction Ordering**: Mempool transactions applied before canonical ordering
4. **Error Recovery**: No rollback if validation fails post-DAG-insert
5. **Orphan Handling**: No handling of competing valid chains in single block

---

## 5. DAG STRUCTURE (`src/core/dag/`)

### Files & Purposes
| File | Purpose |
|------|---------|
| `dag.rs` | Block storage, parent/child links, ancestry queries |
| `block.rs` | BlockNode structure (parents, children, blue/red sets) |
| `anticone.rs` | Anticone computation (unrelated blocks for GHOSTDAG) |
| `mod.rs` | Module exports |

### Key DAG Methods
| Method | Algorithm | Time |
|--------|-----------|------|
| `get_ancestors()` | DFS stack traversal with visited set | O(n) |
| `get_descendants()` | DFS from block forward | O(n) |
| `is_ancestor()` | DFS search for target | O(n) |
| `get_anticone()` | Filter: not ancestor, not descendant | O(n²) |
| `get_block()` | HashMap direct lookup | O(1) |
| `add_block()` | Hash insert + parent/child links | O(parents) |

### DAG Properties
✅ **Implemented:**
- **Parent/Child Tracking**: Bidirectional links maintained
- **Ancestor Chain**: Full ancestry queryable via DFS
- **Determinism**: All traversals sort results for consistency
- **Tip Detection**: Automatic tip set maintained via parent/child updates
- **Cycle Prevention**: Block cannot be its own ancestor (validated on add)

### Path Traversal Capabilities
✅ **Full DAG Traversal:**
- Find all ancestors of block
- Find all descendants of block
- Detect ancestor relationship (A is-ancestor-of B)
- Compute anticone (non-related blocks)
- Get all tips (multi-parent blockchain)

### Gaps for Reorg
❌ **Missing:**
1. **Path Comparison**: No "which chain is longer" method (blue_score used instead)
2. **Branch Detection**: No explicit branch tracking (detected via multiple tips)
3. **Distance Metrics**: Only ancestor check, not distance calculation
4. **LCA Cached**: No memoization of common ancestor (recomputed each time)
5. **Ordered Traversal**: ancestry returns HashSet then converts to sorted Vec (inefficient)

---

## INTEGRATION SUMMARY

### What's Ready ✅
- Reorg state machine (detect, validate, execute) fully implemented
- Atomic snapshot-based rollback working
- GHOSTDAG consensus operational per block
- UTXO state applied per block
- Mempool integrated for tx removal

### What's Blocked ❌
| Issue | Location | Impact | Fix Needed |
|-------|----------|--------|-----------|
| Reorg not executed | block_pipeline.rs | **CRITICAL** - reorgs never trigger | Refactor Engine to enable simultaneous (dag, state, ghostdag) borrows |
| Virtual block disconnected | block_pipeline.rs | State apply uses wrong block | Use get_virtual_block() result for state |
| No transaction/state history | Engine | Can't query history | Implement state persistence per block |
| Incremental rollback incomplete | UTXO | snapshot-only workaround | Store spent outputs for delta rollback |
| Mempool sync missing | reorg.rs | Reverted txs not re-entered | Return evicted txs from execute_reorg() |

---

## RECOMMENDED REORG INTEGRATION STEPS

### 1. Fix Borrow Checker (Immediate)
```rust
// Option A: Refactor Engine methods to return refs properly
pub fn process_with_reorg(&mut self, block: BlockNode) -> Result<(), CoreError> {
    // Store old state before reorg attempt
    let state_snapshot = self.state.snapshot();
    
    // Execute reorg with owned refs
    let reorg_needed = detect_reorg(&self.dag, &self.ghostdag, ...)?;
    if let Some(reorg) = reorg_needed {
        execute_reorg(&self.dag, &self.ghostdag, &mut self.state, reorg)?;
    }
}
```

### 2. Integrate into Pipeline
- Call `detect_reorg()` after virtual block computed
- Execute `execute_reorg()` if needed before state_apply
- Update mempool with reverted transactions

### 3. Add State History
- Store BlockchainState snapshot per block height
- Enable historical queries (utxo at height N)
- Support full reorg to any ancestor

### 4. Optimize Paths
- Memoize common ancestor in ReorgState
- Cache blue_score computations
- Consider topological sort for large DAGs
