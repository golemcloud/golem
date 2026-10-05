# Testing

The TUI test strategy should make interactive behavior inspectable without requiring manual terminal use for every change.

## Validation Commands

Use targeted commands for TUI work:

```shell
cargo check -p golem-cli
cargo check -p golem-cli --features tui-preview --example tui-preview
cargo test -p golem-cli --lib -- tui:: --report-time
cargo test -p golem-cli --lib --features tui-preview -- tui:: --report-time
```

Do not use `cargo make test`. Avoid `--report-time` on broad `cargo test` commands because doctests reject it on stable Rust.

In this environment, `test-r` may need local socket listener permissions. If the sandboxed run fails with `Failed to create local socket listener`, rerun the same targeted command outside the sandbox.

The rebuilt local Dev/read-only Ops checkpoint runs the complete `tui::`
library suite in both feature configurations. Preview coverage renders every
registered production fixture at its default, wide, short, narrow, and
degenerate sizes. Both package configurations must compile without TUI
dead-code or unused-import warnings.

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
- Metrics fixtures are introduced only for an active provider-independent UI
  review and must remain unmistakably synthetic;
- stale typed refresh results are ignored by request and context generation;
- non-manifest, unreachable, and incomplete contexts keep unavailable views
  and actions discoverable with concrete reasons;
- only visual preferences survive restart; operational session state does not;
- active-work quit defaults to Return and requires an explicit Force quit for
  every implemented TUI-owned Job/session combination; Live joins this matrix
  when implemented.
# TUI preview verification

The `tui-preview` feature contains the host and a feature-gated production
fixture; normal CLI builds do not compile either. The browser, terminal, and
export modes render registered cases through Ratatui's `TestBackend` and the
production render tree. The browser representation preserves cells, symbols,
colors, and modifiers, although indexed colors and font metrics can vary from a
real terminal.

Use these focused checks when changing the renderer:

```shell
cargo check -p golem-cli
cargo check -p golem-cli --features tui-preview --example tui-preview
cargo test -p golem-cli --lib -- tui::
cargo test -p golem-cli --lib --features tui-preview -- tui::
```

Also inspect `cargo make tui-preview` for browser reload behavior and
`cargo make tui-preview-terminal` for case switching and resize cleanup. Use
`cargo make tui-preview-live` only for the production provider and PTY path.

Browser review links preserve case, font, and size across watched server
restarts. Every registered case has a unique ID. The persistent catalog contains
only `ops-agents`; add a focused lab case only for an active design question,
and remove it after acceptance unless it adds unique regression coverage.

The preview wrapper is tested against direct production rendering. The
production case renders at representative wide, short, narrow, and `1×1`
sizes and must reject unknown case IDs.

Pane geometry tests assert that every visible scrollbar occupies its pane's
trailing-right cell, removes exactly one usable content column, preserves the
outer-left spine, stays distinct from resize dividers, and exposes nested
horizontal resize handles through hit testing. Scrolling story tests additionally
assert that text and ellipses stop immediately before that reserved cell.
Terminal preview tests route resize events through a backend-generic helper and
verify both viewport dimensions and redraw signaling.

Pane-table tests cover required and optional visibility, transactional chooser
apply/cancel behavior, content-aware width allocation, horizontal offset
clamping, frozen marker placement, selected-row wrapping, one-cell selection
rails, and all four odd/even × selected/unselected surface tokens. Fitting and
truncation tests include wide, emoji, and combining characters and assert exact
terminal display widths.

The watched terminal preview uses up/down for registered case inspection. Its
process must handle the watcher's termination signal, poll for input with a
bounded delay, and return through the normal terminal cleanup guard before
cargo-watch rebuilds it. Do not apply this restart model to the production live
runtime.

Browser preview fonts are pinned remote assets and require network access in
the reviewing browser. A font must load at regular and bold weights and pass the
terminal-grid glyph checks before frames are shown. Font loading or metric
failure must block visual review instead of silently using a platform fallback.
When typography changes, manually check descenders and vertically joined rails
at every selectable size in a browser reached through the normal port-forwarded
workflow.
