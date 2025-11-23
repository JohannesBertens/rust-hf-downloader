# Verification Progress Bar Correctness Analysis

## Problem Statement
When multiple verifications run in parallel, we need to ensure that:
1. Each verification updates its own progress bar
2. Completing verifications don't corrupt ongoing ones
3. The UI always shows consistent state

## Solution: Filename-Based Identification

### Previous Approach (Buggy - Index-Based)
```rust
// Store index when starting
let progress_index = progress.len() - 1;

// Later update using stored index
progress[progress_index].verified_bytes = bytes_verified;  // WRONG!
```

**Problem**: If another verification finishes and removes itself using `retain()`, the indices shift and we update the wrong entry or crash.

**Example Race Condition**:
```
Time 0: Vec = [file1, file2, file3]
        file1 gets index 0
        file2 gets index 1
        file3 gets index 2

Time 1: file1 finishes, removes itself
        Vec = [file2, file3]
        
Time 2: file3 tries to update index 2
        CRASH: index 2 doesn't exist!
        OR updates wrong file if new verification started
```

### Current Approach (Correct - Filename-Based)
```rust
// Find and update by filename
if let Some(entry) = progress.iter_mut().find(|p| p.filename == filename) {
    entry.verified_bytes = bytes_verified;
    entry.speed_mbps = speed;
}
```

**Why It Works**:
- Each verification identifies itself by filename (unique identifier)
- Removing other entries doesn't affect the search
- If the entry is removed, `find()` returns `None` and update is skipped (safe)

## Data Flow Architecture

### 1. Starting Verification
```rust
// verify_file() in src/verification.rs
{
    let mut progress = verification_progress.lock().await;
    progress.push(VerificationProgress {
        filename: item.filename.clone(),  // Unique identifier
        // ...
    });
}
```

### 2. Updating Progress
```rust
// calculate_sha256_with_progress() in src/verification.rs
let mut progress = verification_progress.lock().await;
if let Some(entry) = progress.iter_mut().find(|p| p.filename == filename) {
    entry.verified_bytes = bytes_verified;  // Updates correct entry
    entry.speed_mbps = speed;
}
```

### 3. Finishing Verification
```rust
// verify_file() in src/verification.rs
{
    let mut progress = verification_progress.lock().await;
    progress.retain(|p| p.filename != item.filename);  // Safe removal
}
```

### 4. Rendering UI
```rust
// App::draw() in src/ui/app.rs
let ver_prog = self.verification_progress.lock().await.clone();  // Snapshot

// render_verification_progress() in src/ui/render.rs
for (i, ver) in verifications.iter().enumerate() {
    // Render using ver.filename for title
    // Percentage calculated from ver.verified_bytes / ver.total_bytes
}
```

## Thread Safety Guarantees

### 1. Mutex Protection
- All access to `verification_progress: Arc<Mutex<Vec<VerificationProgress>>>` is protected
- Each operation acquires the mutex, performs atomic update, releases

### 2. Snapshot-Based Rendering
- UI clones the entire Vec before rendering
- Concurrent modifications don't affect the rendered frame
- Each frame shows a consistent snapshot

### 3. Search-Based Updates
- No stored indices that can become invalid
- Each verification finds itself in the Vec every time
- If entry is removed, update is safely skipped

## Test Scenarios

### Scenario 1: Two Parallel Verifications
```
Queue: [file_A.gguf, file_B.gguf]

T0: Worker dequeues file_A, adds to progress Vec
    Vec = [{filename: "file_A.gguf", verified: 0, total: 1GB}]

T1: Worker dequeues file_B, adds to progress Vec
    Vec = [{filename: "file_A.gguf", verified: 0, total: 1GB},
           {filename: "file_B.gguf", verified: 0, total: 2GB}]

T2: file_A updates progress to 500MB
    Vec = [{filename: "file_A.gguf", verified: 500MB, total: 1GB},
           {filename: "file_B.gguf", verified: 0, total: 2GB}]

T3: file_B updates progress to 800MB
    Vec = [{filename: "file_A.gguf", verified: 500MB, total: 1GB},
           {filename: "file_B.gguf", verified: 800MB, total: 2GB}]

T4: file_A finishes, removes itself
    Vec = [{filename: "file_B.gguf", verified: 800MB, total: 2GB}]

T5: file_B updates progress to 1.5GB
    find(filename == "file_B.gguf") -> Success
    Vec = [{filename: "file_B.gguf", verified: 1.5GB, total: 2GB}]  ✓ Correct!
```

### Scenario 2: Verification Finishes During Draw
```
T0: UI starts draw()
    Locks mutex, clones Vec
    snapshot = [{filename: "file_A.gguf", verified: 500MB, total: 1GB}]
    Releases mutex

T1: Verification finishes file_A
    Locks mutex, removes entry
    Vec = []
    Releases mutex

T2: UI renders using snapshot
    Renders file_A at 500MB  ✓ Consistent snapshot, no corruption
```

### Scenario 3: Queue Re-ordering (Not Applicable)
The queue is FIFO (first-in, first-out). Items are removed from the front using `queue.remove(0)`. Re-ordering doesn't happen. Even if it did, filename-based identification would handle it correctly.

## Conclusion

✅ **Correct**: Each verification always updates its own progress bar by finding itself via filename  
✅ **Safe**: No index-based tracking that can become invalid  
✅ **Consistent**: UI works with snapshots, never sees partial updates  
✅ **Robust**: Handles concurrent starts, updates, and completions correctly  

The implementation is thread-safe and race-condition-free.
