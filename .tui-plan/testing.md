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

The rebuilt local Dev/read-only Ops checkpoint currently establishes this
automated baseline:

- 142 focused `tui::app::tests`, including exact nested-CLI arguments,
  one-shot `--yes`, stale typed results, responsive layouts, and interaction
  routing;
- 222 complete `tui::` tests with `tui-preview`, including every deterministic
  story at wide, short, narrow, and extreme sizes;
- `cargo check -p golem-cli --features tui-preview`, package formatting, and
  `git diff --check`.

This does not replace the `UX-024` real-terminal and dev-release playground
acceptance pass.

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

- help, footer, leader, and palette surfaces match registered action labels,
  paths, availability reasons, and risk metadata;
- ctrl+x w/v/l/a/j/e opens the correct hierarchical group and scales without
  assigning top-level numeric shortcuts to destinations;
- ctrl+p and ctrl+x a switch an open palette between All and Context without
  losing its query;
- tab and shift+tab move pane focus only, including while Layout mode selects
  and resizes physical dividers;
- Dev restores primary view, visible subviews, right/bottom placement, focus,
  ratios, persistent Server, and persistent REPL after an Ops round trip;
- direct navigation focuses a visible Dev view and replaces the primary when
  the target is hidden without increasing the pane count;
- Jobs retain immutable launch targets, remain globally visible, and enforce
  one mutating Job per environment with no implicit queue;
- nested Plan/Build/Deploy/Clean, Server, and interactive REPL sessions receive
  the actual pane content size, preserve ANSI, expose 2,000 lines of scrollback,
  and never spawn a real child process from a unit test;
- local and remote resource actions exercise availability, target review,
  remote unlock, risk confirmation, masking, cancellation, and stale-result
  handling;
- context switching applies the accepted stop/preserve/close rules separately
  to Jobs, Server, REPL, and Live;
- Ops root subjects, drill-down subjects, breadcrumbs, direct palette jumps,
  collection/details focus, and Enter-to-open semantics remain consistent;
- read-only Deployment, Environment, and Server refreshes use typed data,
  ignore stale generations, retain stale snapshots on failure, and never
  create a missing application or environment;
- Activity scope is an explicit bounded snapshot, Timeline and Journal retain
  AgentID plus per-Agent oplog index, and Live never silently retargets;
- Metrics navigates family to concrete series, distinguishes service and Agent
  OTLP sources, and renders every provider-availability state truthfully;
- deterministic fake Metrics data is available only to preview/test providers;
- stale typed refresh results are ignored by request and context generation;
- non-manifest, unreachable, and incomplete contexts keep unavailable views
  and actions discoverable with concrete reasons;
- only visual preferences survive restart; operational session state does not;
- quit behavior covers every active Job and TUI-owned session combination once
  the quit-policy planning gate is closed.
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
generic cases intentionally avoid product-state assumptions. Workflow stories
are added for the Dev and Ops checkpoints only after their planning gates close.

Pane geometry tests assert that every visible scrollbar occupies its pane's
trailing-right cell, removes exactly one usable content column, preserves the
outer-left spine, stays distinct from resize dividers, and exposes nested
horizontal resize handles through hit testing. Scrolling story tests additionally
assert that text and ellipses stop immediately before that reserved cell.
Terminal preview tests route resize events through a backend-generic helper and
verify both viewport dimensions and redraw signaling.

Pane-table tests cover required and optional visibility, persisted visual
preferences, transactional chooser apply/cancel behavior, content-aware width
allocation, horizontal offset clamping, frozen marker placement, one-line
ellipsis, one-cell selection rails, and independent minimal/rule/zebra
decorations. Preview coverage includes right/bottom table-details splits and
pane focus at standard and degenerate terminal sizes. Fitting and truncation
tests include wide, emoji, and combining characters and assert exact terminal
display widths.

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
