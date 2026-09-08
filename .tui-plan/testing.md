# Testing

The TUI test strategy should make interactive behavior inspectable without requiring manual terminal use for every change.

## Validation Commands

Use targeted commands for TUI work:

```shell
cargo check -p golem-cli
cargo test -p golem-cli --features tui-preview --lib -- tui::preview --report-time
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
`cargo make tui-preview-terminal` for case/variant switching. Use
`cargo make tui-preview-live` only for the production provider and PTY path.

Browser review links must preserve Focus/Production/Coverage mode and the
relevant focus, case, variant, font, and size across watched server restarts.
Focus mode must show exactly Frame Base plus the current mutations; secondary
tools remain collapsed by default. Every preview case needs a unique ID.

The initial catalog is a workflow-neutral design lab: shell, content density
and states, content splits, content scrolling, long/narrow content, mixed splits
and focus, leader shortcuts, search overlays, and decision overlays. Each case
renders under Production, Frame Base, and the
current focus mutations. Add a
workflow case only when that flow is actively being reviewed, and retain it
after acceptance only when it provides useful regression coverage. Add compact
coverage after normal-sized foundation rules settle.

Production wrapper equivalence is tested separately from the design lab. The
generic cases intentionally do not construct representative Home, Dev, or Ops
application states.

Pane geometry tests assert that every visible scrollbar occupies its pane's
trailing-right cell, removes exactly one usable content column, preserves the
outer-left spine, and stays distinct from resize dividers. Scrolling story
tests additionally assert that text and ellipses stop immediately before that
reserved cell. Terminal preview tests route resize events through a backend-
generic helper and verify both viewport dimensions and redraw signaling.

Pane-table tests cover required and optional visibility, transactional chooser
apply/cancel behavior, horizontal offset clamping, frozen marker placement,
unselected ellipsis, selected-only wrapping, full-height selection surfaces,
continuous full-height `▌` selection rails, and independent minimal/rule/zebra decorations. Preview coverage includes the
table/details split and all four pane-table focus stories at standard and
degenerate terminal sizes.

Decoration tests also assert unpadded one-cell rules and all four odd/even ×
selected/unselected surface tokens. The Columns overlay coverage checks its
popup-table headers, selected row, visibility states, and navigation hint.

The terminal design lab is watched. It starts on the current focus case,
left/right cycles only the focus options, `p` toggles the Production reference,
and up/down remains available for secondary case inspection. Its process must
handle the watcher's termination signal, poll for input with a bounded delay,
and return through the normal terminal cleanup guard before cargo-watch rebuilds
it. Do not apply this restart model to the production live runtime.

Browser preview fonts are pinned remote assets and require network access in
the reviewing browser. A font must load at regular and bold weights and pass the
terminal-grid glyph checks before frames are shown. Font loading or metric
failure must block visual review instead of silently using a platform fallback.
When typography changes, manually check descenders and vertically joined rails
at every selectable size in a browser reached through the normal port-forwarded
workflow.
