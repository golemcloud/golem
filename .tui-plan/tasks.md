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

- [x] Introduce Home, Dev, and Ops as the only top-level workspaces.
- [x] Remap Dashboard into Home.
- [x] Remap Output, Server, and REPL into Dev panels and jobs.
- [x] Remap Agents into the new workspace model, preserving its current list, filter, details, refresh, and inspect behavior during the transition.
- [x] Retain the initial TUI context for this milestone; full selected-context switching comes later.

### Milestone 3: Selected Context UX

- [x] Add first global selected-context display and picker foundation.
- [x] Add immutable launch-context display for jobs and provider requests.
- [x] Treat local server as a global TUI-managed service, visible from Dev but not owned by the selected context.
- [x] First selected-context switching policy: block switches with a reason while context-bound jobs are active and leave global services running.
- [x] Replace the flat `TuiContextCandidate` model with a scoped target model that separates server target, app/environment scope, and launch selector provenance.
- [x] Derive available server targets from the resolved launch selector inputs carried by `GolemCliGlobalFlags`: `--environment`, `--local`, `--cloud`, `--profile`, manifest path/discovery flags, config dir, presets, dev mode, and other retained global selector fields.
- [x] Build server target options from the union of manifest environment servers, built-in local/cloud servers, configured profiles, and launch-selector-implied targets.
- [x] Preserve overlapping server endpoints as selectable source/auth variants rather than collapsing them silently.
- [x] Add server-first Ops selection: choose a server target, list app environments visible on that server with direct typed calls, then select an app/environment for Ops.
- [x] Enable Dev only for manifested app contexts, or for server app-environment contexts that match the current manifest app/environment.
- [x] Add confirmation for context switches that affect running context-bound jobs.
- [x] Keep local server running across context switches and use launch-scoped args for server start/restart.
- [ ] Replace `GolemCliGlobalFlags` as the long-term selected-context representation with a TUI target descriptor that can materialize either an `Arc<Context>` or direct clients.

### Milestone 3b: Pointer-Native Layout Hardening

- [x] Add a dedicated TUI layout module that computes named regions and hit-test snapshots for header tabs, footer, workspace body, Dev panels, Ops panes, modal actions, split handles, and the global server drawer.
- [x] Store the latest rendered layout snapshot on `TuiApp` and route pointer events through region IDs rather than current keyboard focus.
- [x] Make mouse wheel routing position-aware so scrolling targets Output, Server, server drawer, or inspect pane under the pointer.
- [x] Let pointer clicks activate workspace tabs, Dev panels, Ops agent rows, inspect panes, context picker rows, and context confirmation/cancel regions.
- [x] Add mouse-resizable split boundaries for Dev primary/secondary splits and the server drawer.
- [x] Add four session-only Dev layout presets: right, left, top, and bottom.
- [x] Add `ctrl+x l` and palette action support for cycling Dev layout presets.
- [x] Choose the global local server v1 surface: a toggleable right-side drawer available from Home, Dev, and Ops, not a top-level Server workspace.
- [x] Add `ctrl+x v` and palette action support for toggling the global server drawer.
- [x] Keep resize ratios session-only and clamp split sizes so panels cannot collapse below usable dimensions.
- [x] Avoid duplicate large server logs by skipping the Dev Server panel while the global drawer is open.

### Milestone 4: Direct Streaming Ops Providers

- [ ] Replace Agent inspect oplog nested CLI with a direct streaming provider.
- [ ] Replace Agent inspect stream nested CLI with a direct streaming provider.
- [ ] Keep view-owned request construction, stale result handling, focus, and event mapping.
- [ ] Track any remaining Ops nested CLI use as transitional debt.

### Milestone 5: Module Split

- [ ] Split stable action metadata and availability logic out of `app.rs`.
- [ ] Split help derivation and interaction documentation helpers out of `app.rs`.
- [ ] Split selected context, job lifecycle, layout, and Ops view state modules only after behavior has settled.

### Milestone 6: Ops-First Production Rebuild

- [x] Choose Ops as the only initial production workspace and retain `[Ops]`
  without an `alt`+number binding.
- [x] Choose Agents as the initial subject and Overview/Metrics as production
  views.
- [x] Keep Activity as one future view with Timeline and Journal modes.
- [x] Promote the accepted Frame Base tokens and widgets into production and
  remove the obsolete visual comparison baseline.
- [x] Rebuild live Agents Overview with the pane table, details, column chooser,
  filtering, explicit agent inclusion, and typed refresh.
- [x] Separate exact server dataset filtering from literal loaded-row Find and
  keep both scopes visible in the Agents collection header.
- [x] Add cursor-backed incremental loading at 200 rows total across the
  selected dataset, with truthful loaded/more-available status and an explicit
  Load Next 200 action.
- [x] Reserve bare printable keys for focused-pane input, move commands behind
  modifiers or the `ctrl+x` leader, and let typing in Agents start loaded-row
  Find directly.
- [x] Show instance metadata before agent-type metadata in Agent details.
- [x] Scope bulk All/None inclusion to loaded Find matches without clearing
  hidden explicit selections.
- [x] Make Agent details focusable, keyboard/mouse scrollable, and resizable;
  reuse structured AgentID formatting and colored JSON metadata.
- [x] Extract reusable collection query/status, semantic cell, pane status,
  scrollable document, JSON, and resizable split primitives.
- [x] Extract cursor continuation/depth state and visual-row table windowing;
  use the rendered viewport for panning and pointer hit testing.
- [x] Expose the live operational columns from the CLI list, keep Agent rows
  single-line with responsive ellipsis widths, and preserve inclusion markers,
  zebra parity, and the vertical viewport independently from selection.
- [x] Reserve `tab`/`shift+tab` for pane focus, use `ctrl+x v` for the Ops view
  and `ctrl+x f` for the local Find field, and avoid `ctrl`+arrow and
  `alt`+number bindings.
- [x] Align Help and Commands into fixed-height rows and strip ANSI escapes
  from agent/server errors before semantic TUI rendering.
- [x] Invalidate stale refreshes on dataset changes; keep continuation explicit
  so keyboard, pointer selection, and mouse-wheel movement never fetch rows.
- [x] Keep modified shortcuts available while loaded-row Find owns printable
  input, and expose details focus/resize controls in contextual footer hints.
- [x] Route mouse-wheel input over the Agents list to row movement without
  stealing details-pane scrolling or triggering network requests.
- [x] Migrate every reachable production popup to the accepted shared overlay,
  notice, selection, and shortcut primitives.
- [x] Add a reachable deterministic fake OTLP Metrics explorer for UI review;
  label both panes as fake and state that no receiver or query store is read.
- [x] Remove Home, Dev, drawer, REPL, output, server, and nested inspect actions
  from the visible shell until their rebuilt workflows exist.
- [x] Keep Activity out of production until a direct typed provider implements
  the agreed scope and ordering rules.

### Milestone 7: Local Observability For GOL-162

- [x] Record the local embedded receiver, dedicated store, deployment-overlay,
  and external-exporter decisions from the discovery work in issue #3456.
- [ ] Validate agent-to-local-router OTLP delivery and capture real payload and
  batch shapes.
- [ ] Define series identity, retention, pruning, and database-size limits.
- [ ] Define discovery, range, aggregation, and availability query contracts.
- [ ] Implement the receiver/store only after those contracts are accepted.

## Deferred Goal: DX/UX Hardening

- [x] Add a Production-first, linkable real-renderer review surface and a
  long-lived preview-case review ledger under `.tui-plan`.
- [x] Replace the workflow-first preview catalog with six focused generic cases
  for shell, content density, splits/focus, leader shortcuts, search, and
  confirmation.
- [x] Narrow visual comparison to the Production baseline and one actively
  iterated Frame Base direction.
- [ ] Review deterministic cases one at a time and record decision-ready
  feedback in `dx-ux-review.md`.
- [ ] Review and promote foundation batches in order: tokens, shell geometry,
  content density, panels/splits, shortcuts, and overlays.
- [x] Accept shared content primitives across single-pane, split-pane,
  scrolling, and long-content stories, retaining compact tables for popups and
  simple summaries.
- [x] Review pane data tables across decoration, long/panned content, selected
  details, and column-chooser stories; main panes use odd/even rows with a
  restrained amber selection fill and accent-colored full-height rail.
- [x] Add the compact shell case after the normal-sized foundation is coherent
  and use it as the responsive acceptance gate.
- [x] Inventory user goals after the foundation is accepted and redefine
  navigation, workspace grouping, and workflows before adding flow stories.
- [ ] Implement explicitly approved feedback as small themed batches and keep
  the design system, tasks, progress, and tests synchronized.
- [x] Remove the Braille Golem logo from the Home background so previews and
  terminals do not depend on platform-specific glyph fallback.
- [ ] Decide whether the Golem logo should return and, if so, choose its
  workspace, size, and terminal-safe representation through DX/UX review.
- [ ] Define the source-of-truth format for documenting actions, shortcuts, scopes, availability rules, and help text after the broader design system settles.
- [ ] Decide which user-facing surfaces are generated from that source: help, palette metadata, footer hints, docs, and test expectations.
- [ ] Add drift checks so shortcut/action documentation cannot silently diverge from the registered TUI actions.
- [ ] Review interaction conventions for leader actions, raw controls, focus modes, details panels, refresh-heavy ops views, tabs, and splits.
- [ ] Keep this behind the current fundamentals and the accepted TUI design system.

## Ongoing Validation

- [ ] Keep `cargo check -p golem-cli` green after TUI changes.
- [ ] Keep `cargo test -p golem-cli --lib -- tui::` green after TUI changes.
- [ ] Manually smoke test installed TUI after terminal lifecycle, PTY, or large UX changes.
