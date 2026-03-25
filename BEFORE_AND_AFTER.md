# Before & After: Borrowing Fixes

## Issue #1: E0502 - Immutable Borrow During Mutation

### BEFORE (Broken)
```rust
fn get_or_compute(&mut self, graph: &TxGraph, tx_id: &Hash) -> Result<&CachedPackage, CoreError> {
    if let Some(cached) = self.cache.get(tx_id) {
        if cached.version == self.version {
            return Ok(cached);  // ← Returns borrowed reference keeping self.cache borrowed
        }
    }
    
    let package = self.build_package_bounded(graph, tx_id)?;
    let score = PackageScore::new(package.total_fee, package.total_size)?;
    
    let cached = CachedPackage {
        package,
        score,
        version: self.version,
    };
    
    self.cache.insert(tx_id.clone(), cached);  // ← ERROR E0502: Can't mutate while borrowed!
    Ok(self.cache.get(tx_id).unwrap())
}

fn select_transactions(&mut self, graph: &TxGraph) -> Result<Vec<Transaction>, CoreError> {
    for tx_id in &candidate_tx_ids {
        let cached = self.package_cache.get_or_compute(graph, &tx_id)?;
        // cached is still borrowed from self.package_cache!
        candidates_data.push((
            tx_id.clone(),
            cached.score,                      // ← Borrow still active
            cached.package.total_size,         // ← Borrow still active
            cached.package.tx_ids.clone(),
        ));
    }
    
    // ERROR: Can't call invalidate_tx - cache still mutably borrowed by candidates_data references
    for tx_id in &package.tx_ids {
        self.package_cache.invalidate_tx(tx_id);  // ← E0502!
    }
}
```

### AFTER (Fixed)
```rust
fn get_or_compute(&mut self, graph: &TxGraph, tx_id: &Hash) -> Result<(PackageScore, u64, HashSet<Hash>), CoreError> {
    // ↓ Changed return type to owned data instead of borrowed reference
    
    if let Some(cached) = self.cache.get(tx_id) {
        if cached.version == self.version {
            // Return owned data - not borrowing anymore
            return Ok((cached.score, cached.package.total_size, cached.package.tx_ids.clone()));
        }
    }
    
    let package = self.build_package_bounded(graph, tx_id)?;
    let score = PackageScore::new(package.total_fee, package.total_size)?;
    
    let cached = CachedPackage {
        package: package.clone(),
        score,
        version: self.version,
    };
    
    self.cache.insert(tx_id.clone(), cached);  // ← Now safe - no borrowing conflict!
    
    // Return owned data
    Ok((score, package.total_size, package.tx_ids))
}

fn select_transactions(&mut self, graph: &TxGraph) -> Result<Vec<Transaction>, CoreError> {
    let mut candidates_data = Vec::new();
    {
        // Scoped block - borrows end here
        for tx_id in &candidate_tx_ids {
            let tx_id_clone = tx_id.clone();
            let (score, package_size, package_tx_ids) = 
                self.package_cache.get_or_compute(graph, &tx_id_clone)?;
            // ↑ get_or_compute returns owned data - no borrowing!
            
            candidates_data.push((
                tx_id_clone,
                score,                 // ← Owned, not borrowed
                package_size,          // ← Owned, not borrowed
                package_tx_ids,        // ← Owned, not borrowed
            ));
        }
    } // Scope ends - any borrows that were active are now released
    
    // Safe to mutate cache now
    for tx_id in txs_to_invalidate {
        self.package_cache.invalidate_tx(&tx_id);  // ← No conflict!
    }
}
```

## Issue #2: E0507 - Cannot Move Out of Shared Reference

### BEFORE (Broken)
```rust
fn topological_sort(&self, txs: &mut Vec<Transaction>, graph: &TxGraph) -> Result<(), CoreError> {
    // ... build indegree map ...
    
    let mut queue = VecDeque::new();
    for (&tx_id, &deg) in &indegree {
        //   ↑ Destructuring &tx_id
        //   ↑ Hash doesn't implement Copy trait
        if deg == 0 {
            queue.push_back(tx_id);  // ← ERROR E0507: Move out of shared reference!
        }
    }
    // ...
}
```

### AFTER (Fixed)
```rust
fn topological_sort(&self, txs: &mut Vec<Transaction>, graph: &TxGraph) -> Result<(), CoreError> {
    // ... build indegree map ...
    
    let mut queue = VecDeque::new();
    for (tx_id, &deg) in &indegree {
        //   ↑ Don't destructure - tx_id is &Hash
        //   ↑ Now we have a reference, not moving
        if deg == 0 {
            queue.push_back(tx_id.clone());  // ← Explicit clone for ownership
        }
    }
    // ...
}
```

## Issue #3: Mixing Phases

### BEFORE (Risky)
```rust
pub fn select_transactions(&mut self, graph: &TxGraph) -> Result<Vec<Transaction>, CoreError> {
    let ready_txs = graph.get_ready_txs()?;
    
    let candidates = self.pre_filter_candidates(&ready_txs);
    
    // Phase 1&2&5 mixed: Compute, process, invalidate in same scope
    let mut heap = BinaryHeap::new();
    for tx_id in &candidates {
        let cached = self.package_cache.get_or_compute(graph, &tx_id)?;
        heap.push(SelectionCandidate {
            tx_id: tx_id.clone(),
            score: cached.score,
            package_size: cached.package.total_size,
        });
        // cached borrow ends here but...
        self.package_cache.invalidate_tx(&tx_id);  // ← Invalidating during iteration?
    }
    // ...
}
```

### AFTER (Clean Phases)
```rust
pub fn select_transactions(&mut self, graph: &TxGraph) -> Result<Vec<Transaction>, CoreError> {
    let ready_txs = graph.get_ready_txs()?;
    
    // PHASE 1: Collect
    let candidate_tx_ids = self.pre_filter_candidates(&ready_txs);
    
    // PHASE 2: Compute (in scoped block)
    let mut candidates_data = Vec::new();
    {
        for tx_id in &candidate_tx_ids {
            let (score, size, tx_ids) = self.package_cache.get_or_compute(graph, &tx_id.clone())?;
            candidates_data.push((tx_id.clone(), score, size, tx_ids));
        }
    } // Scope ends - borrow released
    
    // PHASE 3: Build
    let mut heap = BinaryHeap::new();
    for (tx_id, score, size, _) in &candidates_data {
        heap.push(...);
    }
    
    // PHASE 4: Select
    let mut txs_to_invalidate = Vec::new();
    while let Some(candidate) = heap.pop() {
        // ... selection logic ...
        txs_to_invalidate.push(tx_id.clone());
    }
    
    // PHASE 5: Invalidate (safe - all reads complete)
    for tx_id in txs_to_invalidate {
        self.package_cache.invalidate_tx(&tx_id);  // ← Now safe!
    }
    
    // PHASE 6: Order
    self.topological_sort(&mut selected_txs, graph)?;
    
    Ok(selected_txs)
}
```

## Key Lessons

### Lesson 1: Owned Data vs Borrowed References
```rust
// ✗ Don't return borrowed data that keeps self mutably bound
fn bad(&mut self) -> &Data { ... }

// ✓ Return owned data to release borrows immediately
fn good(&mut self) -> Data { ... }
```

### Lesson 2: Scoped Borrow Release
```rust
// ✗ Long-lived borrows prevent mutation
let x = self.data.get(...);
// ... use x ...
self.data.mutate(...);  // ERROR

// ✓ End borrows in scopes
{
    let x = self.data.get(...);
    // ... use x ...
}  // Borrow ends
self.data.mutate(...);  // OK
```

### Lesson 3: Explicit Cloning
```rust
// ✗ Implicit moves cause errors
for item in &vec {
    set.insert(item);  // Move!
}

// ✓ Explicit clones are clear
for item in &vec {
    set.insert(item.clone());  // Clone
}
```

## Rust Ownership Summary

- **Ownership**: One owner at a time
- **Borrowing**: Either many immutable OR one mutable, not both
- **Lifetime**: Borrow ends at last use
- **Safety**: All checked at compile time

The compiler enforces these rules so we don't have to think about memory management bugs at runtime!
