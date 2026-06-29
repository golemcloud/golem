# Testing

The TUI test strategy should make interactive behavior inspectable without requiring manual terminal use for every change.

## Validation Commands

Use targeted commands for TUI work:

```shell
cargo check -p golem-cli
cargo test -p golem-cli --lib -- tui::
```

Do not use `cargo make test`. Avoid `--report-time` on broad `cargo test` commands because doctests reject it on stable Rust.

In this environment, `test-r` may need local socket listener permissions. If the sandboxed run fails with `Failed to create local socket listener`, rerun the same targeted command outside the sandbox.

## Test Layers

- Pure state tests for reducers, action selection, filtering, context selection, and job lifecycle.
- Scoped logging tests for concurrent captures, indentation/output restoration across `.await`, and global fallback behavior.
- Render tests with `ratatui::backend::TestBackend` for important visible states.
- `TuiTestDriver` scenario tests for high-level workflows.
- PTY smoke tests only when terminal lifecycle or nested process behavior changes.

## TuiTestDriver

The driver should support:

- constructing default app state;
- sending key events;
- rendering frame text;
- asserting visible and absent text;
- dumping active view, mode, context, and frame text on failure.

Use the driver for scenario tests that protect manual UX paths. Keep exact snapshots small and rare.

## Scenario Coverage

High-value scenarios:

- help/footer/palette surfaces match registered action labels and shortcuts;
- leader help shows leader-scoped actions;
- agent inspect mode has reachable scoped help;
- REPL focus and leader modes show the correct controls;
- future context switching preserves launch context for running jobs;
- stale typed refresh results are ignored by request generation and context generation;
- non-manifest contexts hide unavailable dev actions with reasons.
