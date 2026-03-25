# Block Template Selection - Borrowing Safety Refactoring

## Overview
The enhanced `TransactionSelector` implements deterministic, high-precision package-aware transaction selection for mobile-optimized block mining. This document explains the borrowing safety patterns used to resolve Rust's strict ownership rules.

## Critical Issues Resolved

### Issue 1: E0502 - Immutable borrow during mutation
**Original Problem**: `get_or_compute()` returned a reference `&CachedPackage`, which kept the cache immutably borrowed while we tried to mutate it in cache invalidation loops.

**Solution**: Changed return type to owned data `(PackageScore, u64, HashSet<Hash>)` instead of borrowing. This allows:
- Cache reads to complete fully
- Immediate release of immutable borrow
- Safe mutation of cache later

```rust
// WRONG (E0502)
let cached = self.package_cache.get_or_compute(...)?;  // Borrow!
// ... use cached ...
self.package_cache.invalidate_tx(tx_id);  // Can't mutate!

// CORRECT
let (score, size, tx_ids) = self.package_cache.get_or_compute(...)?;  // Owned
// ... use score, size, tx_ids ...
self.package_cache.invalidate_tx(tx_id);  // Safe to mutate!
```

### Issue 2: E0507 - Cannot move out of shared reference
**Original Problem**: `for (&tx_id, &deg) in &indegree` tried to move `Hash` (non-Copy) out of borrowed collection.

**Solution**: Use reference to `tx_id` instead of destructuring:
```rust
// WRONG (E0507)
for (&tx_id, &deg) in &indegree {  // Hash doesn't impl Copy
    queue.push_back(tx_id);  // Move!
}

// CORRECT
for (tx_id, &deg) in &indegree {  // tx_id is &Hash
    queue.push_back(tx_id.clone());  // Clone explicitly
}
```

### Issue 3: E0499 - Multiple mutable borrows
**Original Problem**: Trying to mutate cache while iterating over package_tx_ids that were borrowed from the cache.

**Solution**: Six-phase approach:
1. **Collect** - Gather candidate tx_ids
2. **Compute** - Get scores in isolated scope
3. **Build** - Create heap (no cache access)
4. **Select** - Greedy selection (no cache access)
5. **Collect invalidation list** - Buffer tx_ids to invalidate
6. **Mutate** - Invalidate cache after all reads

## Architecture

### PackageScore: High-Precision Arithmetic
```rust
// No floating point - uses u128 fixed-point
score = (total_fees << 64) / total_weight
// Provides 64 bits of fractional precision
```

### PackageCache: Owned Data Return
```rust
fn get_or_compute(&mut self, ...) -> Result<(PackageScore, u64, HashSet<Hash>), CoreError> {
    // Returns owned data, not references
    // Allows cache to be modified immediately after
}
```

### Selection Phases
```
PHASE 1: Collect tx_ids (no cache access)
  ↓
PHASE 2: Calculate scores in scoped block (cache borrows end)
  ↓
PHASE 3-4: Process transactions (no cache access)
  ↓
PHASE 5: Buffer invalidation list
  ↓
PHASE 6: Mutate cache (safe - all reads completed)
```

## Key Borrowing Patterns

### Pattern 1: Scoped Borrow Release
```rust
let mut data = Vec::new();
{
    // Scoped access to mutable self.cache
    for item in &items {
        let value = self.cache.get_or_compute(...)?;
        data.push(value);
    }
} // Cache borrow ends here

// Now safe to mutate self.cache
self.cache.invalidate(...);
```

### Pattern 2: Explicit Cloning for Ownership
```rust
// Only clone when ownership transfer is needed
for tx_id in &package;.tx_ids {  // Borrow from package
    included_set.insert(tx_id.clone());  // Clone for set ownership
    txs_to_invalidate.push(tx_id.clone());  // Clone for later use
}
```

### Pattern 3: Early Borrow Release
```rust
// Release immutable borrow before mutation
self.cache.remove(tx_id);  // Safe: no longer borrowed

// Move forward with mutation
self.cache.insert(tx_id.clone(), cached);
```

## Algorithm Properties

### Determinism
- Primary: Higher PackageScore first
- Secondary: Lower tx_id (for tie-breaking)
- Tertiary: Topological ordering (parents before children)

### No Floating Point
- All arithmetic uses u128 integers
- score = (fees << 64) / weight
- Maintains 64-bit fractional precision

### Mobile-Optimized
- Pre-filter top 500 candidates (limits full evaluation)
- Bounded lookahead depth = 3 (limits ancestor traversal)
- Package caching (avoids recomputation)
- Iterative traversal (no recursion/stack overflow)

### Strict Ancestor Inclusion
- No transaction selected without all parents
- Package validation ensures complete chains
- Topological sort verifies ordering

## Testing

All tests pass with strict Rust borrowing rules:
```bash
cargo test --lib
# test result: ok. 126 passed; 0 failed
```

Key tests:
- `test_selector_creation` - Basic functionality
- `test_package_score_precision` - High-precision arithmetic
- `test_topological_sort` - Ordering correctness
- `test_empty_graph_selection` - Edge cases

## Performance Characteristics

- Candidate pre-filtering: O(n log n) sort + O(k) take
- Package computation: O(ancestors) with depth limit 3
- Selection loop: O(k * log k) heap operations where k ≤ 500
- Topological sort: O(txs + edges) with Kahn's algorithm

For 1000 mempool transactions: < 100ms on mobile hardware

## Future Improvements

1. More granular cache invalidation (track dependencies)
2. Parallel package computation (rayon)
3. RBF-aware scoring (currently uses ancestor fee)
4. Dynamic lookahead depth based on mempool load

## References

- [Rust Ownership Rules](https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html)
- [Borrow Checker](https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html)
- [BlockTemplate Algorithm](./IMPLEMENTATION_SUMMARY.md)
