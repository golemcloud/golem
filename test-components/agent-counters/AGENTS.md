# agent-counters Test Component

This is a Golem Application test component used by worker executor tests.

## Prerequisites

- Rust with `wasm32-wasip1` target
- `cargo-component` 0.21.1
- The `golem` CLI built from the repo (at `target/debug/golem`)

## Building

From within the `test-components/agent-counters/` directory, build with release profile:

```shell
../../target/debug/golem build -P release --force-build
```

Use the locally built golem binary from the repo, not an installed version, to ensure version compatibility.

Then copy the output to the test-components directory:

```shell
cp golem-temp/agents/it_agent_counters_release.wasm ..
```

The compiled WASM file `test-components/it_agent_counters_release.wasm` is checked into the repository.
