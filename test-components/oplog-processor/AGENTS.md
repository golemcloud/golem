# oplog-processor Test Component

This is a Golem Application test component used by integration tests for oplog processor plugin functionality.

## Prerequisites

- Rust with `wasm32-wasip1` target
- The `golem` CLI built from the repo (at `target/debug/golem`)

## Building

From within the `test-components/oplog-processor/` directory, build with release profile:

```shell
../../target/debug/golem build -P release --force-build
```

Use the locally built golem binary from the repo, not an installed version, to ensure version compatibility.

Then copy the output to the test-components directory using the custom command defined in the component's `golem.yaml`:

```shell
../../target/debug/golem exec -P release copy
```

The compiled WASM file `test-components/oplog-processor.wasm` is checked into the repository.
