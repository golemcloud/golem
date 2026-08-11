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
# TUI preview verification

The browser and terminal preview galleries render deterministic `TuiApp`
stories through Ratatui's `TestBackend` and the production render tree. The
browser representation is cell-accurate and preserves symbols, colors, and
modifiers, although indexed colors and font metrics can vary from a real
terminal.

Use these focused checks when changing the renderer:

```shell
cargo check -p golem-cli
cargo check -p golem-cli --features tui-preview --example tui-preview
cargo test -p golem-cli --lib -- tui::
```

Also inspect `cargo make tui-preview` for browser reload behavior and
`cargo make tui-preview-terminal` for story/variant switching. Use
`cargo make tui-preview-live` only for the production provider and PTY path.

Browser review links must preserve the selected preview case and Review,
Compare, or Coverage mode, plus the selected font and size, across watched
server restarts. Every preview case needs a unique ID even when multiple
dimensions reuse the same underlying `TuiApp` fixture.
