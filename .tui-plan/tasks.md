# Tasks

This is the active TUI backlog. Keep it goal-sized and synchronized with `progress.md`.

## Status Markers

- `[ ]` planned
- `[~]` in progress
- `[x]` done
- `[!]` blocked
- `[-]` deferred or dropped

## Completed Goal: Action And Help Rules

- [x] Add action IDs, categories, scopes, execution kind, and palette visibility.
- [x] Derive palette filtering from visible actions and include category/scope/execution text in search.
- [x] Derive footer, leader hints, and help from registered action metadata.
- [x] Keep raw input controls as explicit context-help controls.
- [x] Make Agent inspect help reachable.
- [x] Add focused action/help drift tests and a small test driver.

## Completed Goal: Scoped Logging And Context Executor Foundation

- [x] Introduce a scoped `LogContext` that can capture output without changing global logging for unrelated work.
- [x] Make `LogIndent` and `LogOutput` restore the context they were created against, including across `.await`.
- [x] Add async capture and spawn helpers that propagate logging context and tracing span.
- [x] Add focused logging tests for two concurrent captures, indentation across `.await`, scoped output mode, and global fallback behavior.
- [x] Keep the TUI event loop async and use Tokio event delivery instead of blocking the CLI runtime on a sync event receiver.
- [x] Use a bounded TUI event channel with explicit tick-drop behavior under backpressure.
- [x] Run non-interactive nested Agents refresh with async subprocess APIs instead of `spawn_blocking`.
- [x] Move TUI PTY reader/wait workers from raw threads to Tokio blocking tasks.
- [x] Use async terminal input streams instead of blocking `crossterm::event::read`.
- [x] Queue PTY write/resize/kill control requests through a bounded channel so TUI event handlers do not call `portable-pty` control methods inline.
- [x] Replace the temporary provider shape with a smaller context executor that owns context generation, background execution, and logging scope only.
- [x] Make the context executor reuse the active Tokio runtime handle instead of creating or blocking on its own runtime.
- [x] Remove executor spawn counters and test through observable completion events.
- [x] Keep view-specific request building and `TuiEvent` mapping in the views or app state, not in the executor.
- [x] Remove TUI-specific handler APIs and extract neutral data-returning helpers only when they are useful for both CLI and TUI call paths.

## Completed Goal: Direct Typed Agents Refresh

- [x] Extract a neutral data-returning worker list request/helper.
- [x] Keep existing CLI `agent list` rendering on top of the typed helper.
- [x] Move TUI Agents list refresh from nested JSON CLI output to a typed context-executor call.
- [x] Preserve manual refresh, auto-refresh, mode cycle, fuzzy filtering, details, and stale result protection.
- [x] Remove production JSON parsing fallback for the Agents list.

## Completed Goal: TUI Design System Discovery

- [x] Interview the user about the TUI's intended workflows, navigation model, dev workspace, ops explorer, layouts, and interaction rules.
- [x] Reconcile interview answers with the current implementation: modes, views, actions, jobs, context execution, logging, and remaining nested ops debt.
- [x] Create the initial design system document only after the important open questions are answered.
- [x] Interview the user about shortcut notation, navigation, leader grammar, focus, text input, list navigation, and shortcut conflict rules.
- [x] Add initial interaction and shortcut rules to the design system document.
- [x] Review the document, defer non-blocking open questions, and accept the design system for workspace-shell planning.
- [x] Keep major implementation work paused until the design-system review is accepted; only low-risk metadata cleanup happened before acceptance.

## Milestone Backlog

### Milestone 0: Design Review

- [x] Review `ui-system.md` and record section status.
- [x] Revise the design-system document through interview/review passes.
- [x] Accept the design system before major workspace or navigation reshaping starts.
- [x] Allow only low-risk metadata/alignment cleanup before the review is accepted.

### Milestone 1: Interaction Metadata Cleanup

- [x] Standardize user-facing shortcut notation on lowercase forms such as `ctrl+x`, `ctrl+p`, `esc`, `enter`, and `tab`.
- [x] Fix action metadata drift, including execution kind drift for direct typed Agents refresh.
- [x] Add action availability reasons and show unavailable actions consistently.
- [x] Align help, footer, leader hints, and palette wording with workspace/panel/focus/job vocabulary.
- [x] Keep this milestone behavior-preserving except for clearer disabled-state and help/metadata text.

### Milestone 2: Workspace Shell Full Remap

- [ ] Introduce Home, Dev, and Ops as the only top-level workspaces.
- [ ] Remap Dashboard into Home.
- [ ] Remap Output, Server, and REPL into Dev panels and jobs.
- [ ] Remap Agents into the new workspace model, preserving its current list, filter, details, refresh, and inspect behavior during the transition.
- [ ] Retain the initial TUI context for this milestone; full selected-context switching comes later.

### Milestone 3: Selected Context UX

- [ ] Add global selected-context display and picker.
- [ ] Add immutable launch-context display for jobs and provider requests.
- [ ] Add confirmation for context switches that affect running context-bound jobs.
- [ ] Hide or disable local-server actions with reasons when the selected context does not use a local server.

### Milestone 4: Direct Streaming Ops Providers

- [ ] Replace Agent inspect oplog nested CLI with a direct streaming provider.
- [ ] Replace Agent inspect stream nested CLI with a direct streaming provider.
- [ ] Keep view-owned request construction, stale result handling, focus, and event mapping.
- [ ] Track any remaining Ops nested CLI use as transitional debt.

### Milestone 5: Module Split

- [ ] Split stable action metadata and availability logic out of `app.rs`.
- [ ] Split help derivation and interaction documentation helpers out of `app.rs`.
- [ ] Split selected context, job lifecycle, layout, and Ops view state modules only after behavior has settled.

### Milestone 6: Local Observability

- [ ] Plan the TUI role for local observability from issue #3456.
- [ ] Decide how local/server metrics fit into Dev and Ops Monitor dashboard surfaces.
- [ ] Keep observability as workspace panels and dashboard content, not as a new top-level workspace.

## Deferred Goal: DX/UX Hardening

- [ ] Define the source-of-truth format for documenting actions, shortcuts, scopes, availability rules, and help text after the broader design system settles.
- [ ] Decide which user-facing surfaces are generated from that source: help, palette metadata, footer hints, docs, and test expectations.
- [ ] Add drift checks so shortcut/action documentation cannot silently diverge from the registered TUI actions.
- [ ] Review interaction conventions for leader actions, raw controls, focus modes, details panels, refresh-heavy ops views, tabs, and splits.
- [ ] Keep this behind the current fundamentals and the accepted TUI design system.

## Ongoing Validation

- [ ] Keep `cargo check -p golem-cli` green after TUI changes.
- [ ] Keep `cargo test -p golem-cli --lib -- tui::` green after TUI changes.
- [ ] Manually smoke test installed TUI after terminal lifecycle, PTY, or large UX changes.
