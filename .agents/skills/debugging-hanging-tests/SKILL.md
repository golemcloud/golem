---
name: debugging-hanging-tests
description: "Diagnosing and fixing hanging worker executor or integration tests. Use when a test hangs indefinitely, times out, or appears stuck during execution."
---

# Debugging Hanging Tests

Worker executor and integration tests can hang indefinitely due to `unimplemented!()` panics in async tasks, deadlocks, missing shard assignments, or other async runtime issues. This skill provides a systematic workflow for diagnosing and resolving these hangs.

## Common Causes

| Cause | Symptom |
|-------|---------|
| `unimplemented!()` panic in async task | Test hangs after a log line mentioning the unimplemented feature |
| Deadlock | Test hangs with no further log output |
| Missing shard assignment | Worker never starts executing |
| Channel sender dropped | Receiver awaits forever with no error |
| Infinite retry loop | Repeated log lines with the same error |

## Step 1: Add a Timeout

Add a `#[timeout]` attribute so the test fails with a clear error instead of hanging forever:

```rust
use test_r::test;
use test_r::timeout;

#[test]
#[timeout("30s")]
async fn my_hanging_test() {
    // ...
}
```

Choose a timeout generous enough for normal execution but short enough to fail quickly when hung (30s–60s for most tests, up to 120s for complex integration tests).

## Step 2: Capture Full Output

Run the test with `--nocapture` and save **all output** to a file. The root cause often appears far before the point where the test hangs:

```shell
cargo test -p <crate> <test_name> -- --nocapture > tmp/test_output.txt 2>&1
```

**Important:** Always redirect to a file. The output can be thousands of lines, and the relevant error may be near the beginning while the hang occurs at the end.

## Step 3: Search for Root Cause

Search the saved output file for these patterns, in order of likelihood:

```shell
grep -n "unimplemented" tmp/test_output.txt
grep -n "panic" tmp/test_output.txt
grep -n "ERROR" tmp/test_output.txt
grep -n "WARN" tmp/test_output.txt
```

### What to look for

- **`not yet implemented`** or **`unimplemented`**: An async task hit an unimplemented code path and panicked. The panic is silently swallowed by the async runtime, causing the caller to await forever.
- **`panic`**: Similar to above — a panic in a spawned task won't propagate to the test.
- **`ERROR` with retry**: A service call failing repeatedly, causing an infinite retry loop.
- **Repeated identical log lines**: Indicates a retry loop or polling cycle that never succeeds.

## Step 4: Fix the Root Cause

### If caused by `unimplemented!()`
Implement the missing functionality, or if it's a test-only issue, provide a stub/mock.

### If caused by a deadlock
Look for:
- Multiple `lock()` calls on the same mutex in nested scopes
- `await` while holding a lock guard
- Circular lock dependencies between tasks

### If caused by missing shard assignment
Check that the test setup properly initializes the shard manager and assigns shards before starting workers.

### If caused by a dropped sender
Ensure all channel senders are kept alive for the duration the receiver needs them. Check for early returns or error paths that drop the sender.

## Checklist

1. `#[timeout("30s")]` added to the hanging test
2. Test run with `--nocapture`, output saved to file
3. Output searched for `unimplemented`, `panic`, `ERROR`
4. Root cause identified and fixed
5. Test passes within the timeout
6. Remove the `#[timeout]` if it was only added for debugging (or keep it as a safety net)
