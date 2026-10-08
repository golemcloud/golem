# Progress

## 2026-10-08: Stable Ops Navigation And Typed Metrics Foundation

- Added stable typed Overview, Agents, Oplog, Stream, Metrics, Deployments, and
  Configuration destinations with independent route stacks, direct leader
  navigation, and a horizontally scrollable clickable tab bar.
- Upgraded the local metrics store to schema version 2 with temporality and
  exact source-oplog anchors. Families and series now page in SQL; samples use
  bounded, reset-aware transforms with gauge seeds and approximation metadata.
- Released the built-in OTLP exporter contract as 1.6.0, attached exact oplog
  indexes to emitted metric points, rebuilt and validated the committed WASM,
  and retained the manifest-generated skill updates from the rebuild.
- Added a typed local Agent/Server dashboard endpoint. Agent snapshots expose
  real totals, bounded p50/p95 duration series, invocation rates, rankings,
  memory, resources, and explicit empty states. Server snapshots expose
  reset-aware request/failure stats, histogram p95, current worker gauges, nine
  curated metric groups, freshness, units, and approximation state.
- Rewired the Metrics dashboard to consume typed snapshots instead of guessing
  panels from the currently selected Explore family. Dashboard/Explore and
  Agent/Server changes now reload the corresponding backend view.

Still open in this slice: custom absolute ranges, chart cursor inspection and
expansion, Explore paging/multi-series overlays, and visual axes/gap rendering.

## 2026-10-08: Stable Ops Rebuild Contract

- Replaced the prototype subject-by-view matrix with stable Overview, Agents,
  Oplog, Stream, Metrics, Deployments, and Configuration destinations.
- Locked explicit cross-destination scope handoff, per-destination route/state,
  bounded collection semantics, and shared loading/freshness states.
- Locked local-first Agent and Server dashboards plus Explore, truthful metric
  transforms, bounded approximation, and source-oplog correlation.
- Locked exact bounded Oplog search, typed grouping, a 20-Agent mixed-scope cap,
  and a persistent exact-Agent Stream session.
- Expanded Configuration to lazy typed Environment, Account, and Server
  inventories while retaining a read-only first milestone.
- Recorded Metrics-first implementation checkpoints and explicit deferrals for
  remote metrics, mutation, traces/log storage, rollups, Grafana, and web UI.

## 2026-10-07: Ops Resources, Activity Snapshots, And Local Metrics

- Added subject-aware Ops view navigation. Agents and individual Agents expose
  Overview/Activity/Metrics, Server exposes Overview/Metrics, and Deployment
  and Environment expose Overview/Resources without cycling through invalid
  views.
- Added read-only Deployment and Environment resource catalogs through typed
  clients. Resource kinds open homogeneous item panes and use safe projections;
  secret values are never serialized into TUI rows or details.
- Added direct typed oplog snapshots for Timeline and Journal with the latest
  200 entries per Agent, soft warning above 10 Agents, hard maximum 50, concurrency 4,
  partial-failure reporting, retained AgentID/component/index, and exact JSON
  details. True transient Live remains explicit follow-up and is not imitated
  by polling oplogs.
- Added a default-on, feature-gated local observability service to `golem` with
  five-second service sampling, authenticated OTLP/HTTP JSON Agent ingestion,
  private typed query routes, and a seven-day/1-GiB SQLite store under the local
  server data directory.
- Added `localServer.observability`, run-time CLI override, observability-only
  clean, `.golem/local-server-data` templates, and opt-out comments.
- Added a normal-deploy-only local metrics exporter overlay. Existing deployed
  Agents are left untouched until their next deploy and explicit user exporter
  configuration wins.
- Added production Metrics family→series→chart/details views for local Agents,
  individual Agents, and Server over a fixed Now-15m range with five-second
  refresh. No Docker stack or companion web UI is required for this slice.

Still open in this area: installed-binary exporter smoke testing, Deployment
History/Diff, deeper resource item navigation/actions, server-side Activity
scope resolution and cursors, true Live, and generalized Metrics controls or
remote providers.

## 2026-10-07: Fair Input, Interactive Scrollbars, And Table Polish

- Split terminal/control events from bounded nested-process output and biased
  dispatch toward input and lifecycle events, preventing sustained Server logs
  from starving keyboard and pointer interaction.
- Added pane-owned scrollbar track clicks and thumb dragging for nested
  terminals, Agent rows, Details, Ops documents, and Help without overlapping
  split-resize behavior.
- Favored AgentID in constrained table widths, right-aligned numeric columns,
  and exposed the active Durable/Ephemeral/All mode beside Dataset and Find.
- Kept active-work quit choices side by side, wrapped Server and REPL state in
  one footer status decorator, and made Layout hints specific to Dev or Ops.
- Restored TUI keyboard ownership when workspace tabs are clicked and removed
  repeated ctrl+x handoff instructions from nested-terminal status summaries.
- Made Server and REPL footer statuses toggle their already visible, focused
  panes while retaining the final-visible-pane guard.
- Made Build & Deploy, Server, and REPL launches reveal and focus their owning
  Dev view without promoting it; Server retains TUI input ownership.

Validation:

- `cargo test -p golem-cli --lib -- tui:: --report-time` (253 passed)
- `cargo check -p golem-cli --features tui-preview`
- `cargo fmt --package golem-cli -- --check`
- `git diff --check`
- `cargo make install-golem-dev-release`
- `/home/noise64/.cargo/bin/golem --version`

## 2026-10-07: Dev View And Persistent-Session Interaction Alignment

- Made direct Build & Deploy, Server, and REPL navigation additive: a hidden
  destination is shown and focused without replacing the main view or hiding
  another visible view.
- Added stable independent view toggles plus focused-view promotion, preserving
  the visible set and refusing to hide the final Dev view.
- Reused the downstream pane title row as the complete draggable divider for
  bottom layouts. Clicking without movement focuses the pane; dragging resizes
  it, and the former duplicate heavy horizontal grip is gone.
- Kept command and session lifecycle independent from primary-view promotion
  and sibling visibility. Launch reveals and focuses its owning view; finite
  commands and REPL receive CLI input, while Server starts under TUI input
  ownership.
- Aligned Server and REPL lifecycle actions to their actual states: Start when
  stopped or failed, Stop and Restart while starting or running, and Force Stop
  after a five-second stopping grace period. Server clean remains a reviewed
  start/restart option.
- Added shared clickable Server and REPL status items to the footer in both
  workspaces, with full state at normal widths and compact named glyphs at
  narrower widths. Clicking shows and focuses the corresponding Dev view
  without promoting it or transferring terminal input.
- Updated the product contract, architecture, design system, and task ledger
  to record the locked navigation, layout, lifecycle, and input rules.

Validation:

- `cargo fmt --package golem-cli -- --check`
- `cargo test -p golem-cli --lib -- tui:: --report-time` (245 passed)
- `cargo check -p golem-cli --features tui-preview`
- `git diff --check`
- `cargo make install-golem-dev-release`

## 2026-10-06: Locked Review Editing, Palette Search, And Server Input

- Centered semantic notice lines in content-sized review and quit popups while
  retaining left-aligned form rows. Styled notice wrapping now participates in
  popup height and cursor geometry.
- Added a shared typed review-row primitive and transactional free-text editor
  with visible cursor, input surface, Unicode-safe navigation/deletion,
  horizontal viewport tracking, Enter-to-apply, and esc-to-discard.
- Kept centered Run as the initial primary action with an active orange style,
  placed per-launch confirmation options first, and placed Server clean then
  `--yes` before value fields.
- Made Server ports-file opt-in. Successful process spawn now establishes
  Running independently; an explicitly supplied fresh ports file enriches the
  summary without gating lifecycle or claiming HTTP readiness.
- Server now launches with TUI keyboard ownership and visibly offers Enter to
  hand input to the CLI. Finite commands and REPL retain launch-time CLI input.
- Made pane dividers disjoint in bottom layouts, gave resize regions hit-test
  priority, and rendered a short orange grip while retaining the full neutral
  divider as the mouse target.
- Made ctrl+p reopen the Commands scope last used in the current session while
  ctrl+x a remains an explicit Context entry. Palette results now use stable
  exact/prefix/word-prefix/substring/fuzzy tiers and visibly highlight matched
  characters in labels and metadata.

Validation:

- `cargo fmt --package golem-cli`
- `cargo test -p golem-cli --lib --features tui-preview -- tui:: --report-time`
  (250 passed)
- `cargo test -p golem-cli --lib --features tui-preview -- --report-time`
  (915 passed)
- `cargo check -p golem-cli --features tui-preview`
- `cargo fmt --package golem-cli -- --check`
- `git diff --check`
- `cargo make install-golem-dev-release`
- `/home/noise64/.cargo/bin/golem tui --help`

Real-terminal acceptance remains the final gate for this interaction batch.

## 2026-10-05: Unified Commands And Nested Terminal Ownership

- Collapsed global and contextual command palettes into one Commands overlay
  with All/Context scopes, left/right and mouse switching, and preserved query.
- Made the leader own and clear every footer shortcut row so shorter transient
  menus never reveal unrelated shortcuts behind them.
- Unified finite commands, Server, and REPL around one nested-terminal input
  contract: finite commands and REPL launch into CLI input, Server stays in TUI
  control, ctrl+x returns input to TUI control, and Enter or clicking a running
  pane enters CLI input. Every other key reaches the PTY while CLI input owns
  the keyboard.
- Removed the synthetic stdin row, exposed the real PTY cursor in every nested
  terminal, reserved stable terminal scrollbar geometry, and synchronize PTY
  dimensions after all rendered layout changes.
- Put a centered Run action first in command and session review forms, removed
  internal build-step selection, and size review/quit dialogs from their
  content rather than expanding them to the general data-overlay size.
- Locked active-work quit behavior as safe Return by default with an explicit
  Force quit choice.

Manual terminal acceptance and the later Live-session integration remain open.

Validation:

- `cargo fmt --package golem-cli -- --check`
- `cargo check -p golem-cli --features tui-preview`
- `cargo test -p golem-cli --lib --features tui-preview -- tui:: --report-time`
  (236 passed)
- `git diff --check`
- `cargo make install-golem-dev-release`
- `/home/noise64/.cargo/bin/golem tui --help`

## 2026-10-01: Isolated And Minimized TUI Preview Tooling

- Kept browser, terminal, and static-export preview hosts behind the optional
  `tui-preview` feature, with one deterministic Ops Agents fixture rendered by
  the production Ratatui tree.
- Moved fixture construction into the feature-gated
  `tui::app::preview_fixture` child module so normal CLI builds do not compile
  preview state construction.
- Removed the synthetic story catalog, visual variants, Frame Base and
  candidate machinery, fake OTLP explorer, and obsolete lab-only widget
  contracts. No empty lab module remains.
- Reduced the visual and pane-table APIs to their accepted production
  contracts while retaining odd/even and selected/unselected row coverage.
- Updated the design, architecture, testing, product, and task contracts to
  describe temporary labs as active-review tools rather than a dormant product
  surface. `UX-024` remains open for real-terminal visual acceptance.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo check -p golem-cli --features tui-preview --example tui-preview`
- `cargo test -p golem-cli --lib -- tui:: --report-time` (213 passed)
- `cargo test -p golem-cli --lib --features tui-preview -- tui::preview --report-time`
  (14 passed)
- `cargo test -p golem-cli --lib --features tui-preview -- tui:: --report-time`
  (227 passed)
- `cargo make tui-preview-export`
- `cargo make install-golem-dev-release`
- `/home/noise64/.cargo/bin/golem tui --help`

## 2026-09-29: Hardened TUI State And Runtime Boundaries

- Moved workspace and Dev-view identity out of the application implementation
  so layout and navigation share neutral domain types.
- Replaced ad hoc palette/leader return flags with a tested interaction-mode
  stack. Nested Help, Commands, leader, filters, context selection, and REPL
  now restore the mode that actually opened them.
- Collapsed the parallel action ID/action-kind representation into one typed
  action identity used by registry metadata and execution.
- Replaced nine owner-specific nested-CLI events with one typed process
  envelope and one dispatch point for command, Server, and REPL lifecycle
  traffic.
- Introduced `UiGeometry` as the sole cache for the latest layout snapshot, with
  typed hit-test and region access used by mouse input, pane resize, PTY resize,
  and geometry assertions.
- Kept `UX-024` awaiting manual terminal acceptance; this cleanup does not mark
  the current visual review complete.
- Consolidated selected context identity, nested-CLI arguments, environment
  reference, and Server identity into one `SelectedTarget`. Async switches now
  carry immutable request material through completion rather than consulting
  mutable picker state.
- Bound supervised Server sessions to their launch Server identity. Same-Server
  app/environment switches preserve the session; different-Server switches
  require confirmation, graceful stop, and the existing five-second
  force-stop gate before the switch continues.
- Recomputed context-picker `current` markers from the complete selected target
  after every successful switch, including Server identity when two Servers
  expose the same application/environment names.

Validation:

- `cargo fmt --package golem-cli`
- `cargo test -p golem-cli --lib --features tui-preview -- tui:: --report-time`
  (236 passed)
- `cargo check -p golem-cli --features tui-preview`
- `git diff --check`

## 2026-09-29: Restored The Shared Production Visual Foundation

- Replaced the rebuilt shell's hand-written Dev/Ops header with the accepted
  shared context-only header and its full/compact context grammar.
- Kept the new recursive layouts and workflow behavior while moving Dev pane
  titles, body spines, split junctions, transient leader hints, lifecycle
  summaries, and terminal scrollbars back onto shared foundation primitives.
- Made stacked pane titles own the horizontal resize row, removing adjacent
  double rules without changing stored ratios or resize hit targets.
- Standardized operational state: active/running/success is green, failure is
  red, idle is gray, loading is informational, and orange remains the pane-focus
  accent. Command, Server, and REPL metadata now use compact neutral summary
  rows on the normal pane surface.
- Added buffer-level regressions for identical Dev/Ops headers, compact context,
  pane delimiters, neutral divider cells and junctions, lifecycle colors,
  trailing scrollbars, and wide/short/narrow/extreme recursive layouts.

Validation:

- `cargo fmt --package golem-cli -- --check`
- `cargo test -p golem-cli --lib --features tui-preview tui:: -- --report-time`
  (230 passed)
- `cargo check -p golem-cli --features tui-preview`
- `git diff --check`
- `cargo make install-golem-dev-release`
- `/home/noise64/.cargo/bin/golem tui --help`

## 2026-09-29: Rebuilt Dev Shell And Read-Only Ops Slice

- Replaced the previous product topology with Dev and Ops workspaces, recursive
  right/bottom Dev subviews, reusable Ops collection/details layout, responsive
  shell geometry, hierarchical `ctrl+x` navigation, and switchable All/Context
  command-palette scopes.
- Added reviewed nested-CLI PTYs for Plan, Build, Deploy, Clean, Server, and
  interactive REPL. Each active or completed command/session owns a
  pane-sized `TerminalScreen` with 2,000 lines of scrollback; explicit
  scrollback mode, mouse scrolling, ANSI rendering, five-second force-stop
  gating, and exact argument construction are covered by focused tests.
- Added bounded finite Dev Jobs with the latest 20 completed records, separate
  persistent Server and REPL state, per-context form memory with one-shot
  `--yes`, and conservative quit blocking while TUI-owned work is active.
- Kept Dev execution local and manifest-backed for this slice. Remote and
  ops-only actions remain discoverable with concrete unavailability reasons.
- Migrated the Agents collection into the Ops subject shell without losing its
  Dataset/Find, explicit inclusion, bounded continuation, column, details,
  pointer, keyboard, refresh, and stale-data behavior. Enter now opens a
  read-only individual Agent subject instead of the removed nested Inspect
  path.
- Added current Deployment, Environment, and Server Ops overviews. Environment
  resolution uses a neutral read-only handler that lists/gets existing data and
  never invokes get-or-create behavior; request generations and context IDs
  reject stale completions, and failed refreshes retain the last successful
  snapshot.
- Restricted deterministic fake OTLP data to preview/test builds and made the
  production Metrics surface truthfully report that no provider is configured.
- Recorded action leader path, execution kind, risk, review, target, and
  availability metadata, and persisted visual-only columns, placement, and
  split ratios in the CLI user configuration area.

Checkpoint commit:

- `721390b80 feat(tui): rebuild dev shell and read-only ops views`

Remaining before the next planning session:

- converge the displayed selected-target identity, launch flags, and
  environment reference into one target descriptor;
- enforce stop-or-cancel before switching to a different Server target while a
  supervised Server is active;
- perform final manual terminal acceptance and the dev-release playground
  smoke test;
- keep remote unlock, generalized Ops Jobs/mutations, Activity/Live, and real
  Metrics controls/providers behind their still-open planning gates.

Validation:

- `cargo fmt --package golem-cli -- --check`
- `cargo test -p golem-cli --lib --features tui-preview tui::app::tests -- --report-time`
  (142 passed)
- `cargo test -p golem-cli --lib --features tui-preview tui:: -- --report-time`
  (222 passed)
- `cargo check -p golem-cli --features tui-preview`
- `git diff --check`

## 2026-09-29: Local Dev And Read-Only Ops Contract

- Locked terminal-first nested CLI execution for Plan, Build, Deploy, Clean,
  Server, and interactive REPL.
- Locked typed handler/provider execution for other operations, retaining and
  testing shared CLI cache behavior instead of parsing command output.
- Locked the exact Dev forms, process states, 20-Job history, 2,000-line PTY
  scrollback, five-second graceful-stop window, and process/ports-file Server
  readiness boundary.
- Approved real read-only Deployment, Environment, Server, and individual Agent
  Ops overviews in the same slice as the local Dev rebuild.
- Left extreme shell priorities, final quit behavior, Ops mutations, Activity,
  real Metrics controls/providers, and metrics acquisition for later planning.

## 2026-09-29

- Added `product-contract.md` as the authoritative behavior and readiness
  contract for the rebuilt TUI.
- Locked Dev and Ops as the initial workspaces. Dev owns Build & Deploy, Server,
  and REPL with optional right/bottom subviews; Ops owns resource subjects,
  Activity, Metrics, and contextual resource actions.
- Locked the scalable hierarchical leader, switchable All/Context palette,
  subject/view/pane navigation, finite Jobs versus persistent sessions,
  bounded Activity scope, family-first Metrics model, and remote mutation
  safeguards.
- Rewrote the architecture, phases, and task ledger around reviewable
  implementation checkpoints with explicit planning prerequisites and
  recommended commit boundaries.
- Reconciled the UI-system, testing, and review ledgers with the new contract
  and marked the accepted Ops Agents implementation as the visual and
  interaction baseline rather than the final product topology.
- Removed the previous embedded Metrics receiver/store direction from the
  active plan. Acquisition, retention, Docker integration, and any companion
  web UI are deferred until the provider-independent TUI is accepted.
- Kept seven required planning gates visible: Dev view content, responsive
  shell priorities, quit policy, resource action/risk inventory, Activity
  bounds, Metrics controls, and typed provider boundaries/order.

Validation:

- documentation authority and stale-contract searches
- `git diff --check`

## 2026-09-28

- Replaced the unreliable `alt`+arrow details resize binding with the
  keyboard-layout-safe `ctrl+x` then left/right sequence, including operation
  from loaded-row Find and aligned footer, leader, and Help hints.
- Changed cursor-backed loading to a hard 200-agent total per request across
  the selected dataset. Typed page progress now retains both unfinished
  component cursors and components not yet scanned.
- Made Load Next 200 entirely explicit through `ctrl+l` or Commands. Keyboard
  navigation, pointer selection, and mouse-wheel movement no longer start
  continuation requests; refresh and auto-refresh only reload the depth the
  user has already requested.
- Added case-insensitive literal type-ahead to the Dataset popup over Component
  and Agent type, with a visible match count, `ctrl+u` clear, and the existing
  exact server-side scope applied only on confirmation.
- Animated agent refresh and next-batch loading, and standardized pane status
  shapes to square brackets for active/loading and parentheses for idle.
- Kept popup selection rails visible with an accent foreground independent of
  the selection background.
- Updated the architecture, task, UI-system, and review ledgers to the accepted
  paging, resize, search, marker, and status contracts.

Validation:

- `cargo fmt --package golem-cli`
- `cargo test -p golem-cli --lib --features tui-preview --config 'profile.test.package.golem-cli.debug=0' tui:: -- --report-time` (215 passed)
- Focused global page-budget and explicit-navigation tests (passed)
- `cargo check -p golem-cli --features tui-preview --tests`
- `cargo make install-golem-dev-release`
- Installed `/home/noise64/.cargo/bin/golem` reports
  `golem v1.5.3-301-g6f0c10e4d`.

## 2026-09-26

- Replaced ambiguous notice prefixes with bracketed semantic badges such as
  `[i Notice]` and `[○ Empty]`, keeping glyph color distinct from the muted
  label and message boundary.
- Made Help, Commands, context, Dataset, and Columns data overlays adapt toward
  85 percent of the available terminal while retaining pinned control footers
  and trailing-right scrollbars for overflowing bodies. Confirmations and
  blocking/loading dialogs remain compact.
- Reused the CLI AgentID tokenizer for syntax-colored AgentID cells and Details,
  preserving semantic foregrounds through ellipsis, zebra rows, and selection.
- Added explicit unpadded `│` separators to compact decision tables, including
  the Component/Agent type Dataset selector, and restored the normal pane
  surface across unused rows below short main-pane tables.
- Replaced loaded-row fuzzy `All fields` Find with a case-insensitive literal
  substring search scoped to AgentID by default, Component, or Agent type.
  Matches retain server-loaded order.
- Prevented contextual footer overflow from injecting a second `ctrl+p`; only
  its global owner may fall back to Commands, while other rows use an ellipsis.

Validation:

- `cargo fmt --package golem-cli`
- `cargo test -p golem-cli --lib --features tui-preview --config 'profile.test.package.golem-cli.debug=0' tui:: -- --report-time` (212 passed)
- `cargo test -p golem-cli --lib --features tui-preview --config 'profile.test.package.golem-cli.debug=0' agent_id_display::highlight::tests:: -- --report-time` (13 passed)
- `cargo check -p golem-cli --features tui-preview --tests`
- `cargo make install-golem-dev-release`
- Installed `/home/noise64/.cargo/bin/golem` reports
  `golem v1.5.3-301-g6f0c10e4d`.

## 2026-09-25

- Reserved `tab` and `shift+tab` for pane focus in both Agents Overview and
  the fake Metrics explorer. Overview/Metrics switching now uses `ctrl+x v`,
  local Find-field switching uses `ctrl+x f`, and active Ops navigation no
  longer consumes `alt`+number or `ctrl`+arrow combinations.
- Kept bare `space` as the explicit Columns-overlay exception for toggling an
  optional column; normal pane commands continue to require modifiers while
  printable input belongs to Find.
- Made Agent rows single-line because full values belong in Details. AgentID
  now ellipsizes in the table, responsive column widths expand toward actual
  content when room is available, and the selection rail is always one cell
  high per logical row.
- Persisted the vertical table viewport independently from selection. Pointer
  selection now preserves the clicked row's visible position instead of
  rebuilding the window with that row at the bottom.
- Rebuilt Help and Commands results as left-aligned, fixed-height table rows so
  long labels, shortcuts, descriptions, and unavailable reasons truncate in
  place rather than producing centered or wrapped layouts.
- Stripped ANSI escape sequences at the TUI agent-error boundary so styled CLI
  and server diagnostics render as plain semantic error text.

## 2026-09-24

- Migrated every reachable production popup—Commands, Help, context picker,
  context confirmation, and context loading—to the shared bordered overlay,
  notice, selection-row, and shortcut-row grammar. Dormant legacy Dev/Home
  renderers remain unreachable for later workflow rebuilding.
- Kept modified global and Agent actions active while loaded-row Find owns bare
  typing, including Commands, Help, pane focus/resize, refresh, dataset, and
  loaded-match inclusion actions.
- Made pane focus and `alt+left/right` resize explicit in the
  contextual footer and scoped Help, including direct focus of a previously
  hidden details pane.
- Routed mouse-wheel events over the Agents list to loaded-row selection and
  the same near-end continuation check used by keyboard navigation, while
  preserving independent details-pane scrolling.
- Closed the post-review Agents Overview gaps: the live table now exposes the
  CLI's operational fields as AgentID, Status, Type, Component, Revision,
  Pending, and Created at, with the latter three optional by default.
- Prototyped selected-row wrapping before settling on single-line Agent rows
  during the next review pass.
- Made horizontal panning clamp against the table's actual rendered viewport
  instead of an assumed terminal width.
- Extracted cursor-backed collection depth and continuation bookkeeping into a
  reusable collection state shared by initial loads, continuation requests,
  refreshes, and dataset resets.
- Invalidated in-flight refreshes when the server dataset changes, and made
  mouse selection near the loaded boundary trigger the same continuation path
  as keyboard navigation.
- Kept explicit inclusion markers accented independently of focus and retained
  the full-height selection rail for wrapped rows.
- Reordered Agent details so instance metadata precedes agent-type metadata.
- Reserved bare printable keys for focused-pane input, made typing in the
  Agents list start loaded-row Find, and moved command actions to `ctrl`, `alt`,
  or `ctrl+x` shortcuts. Direct structural keys such as arrows, `enter`, and
  `esc` remain local navigation controls.
- Replaced the Metrics availability placeholder with a deterministic fake OTLP
  series explorer for UI review. Both panes retain `FAKE` labels and the detail
  pane states that no receiver or query store was read.

Validation:

- `cargo fmt --package golem-cli`
- `cargo test -p golem-cli --features tui-preview tui:: -- --report-time`
- `cargo check -p golem-cli --features tui-preview --tests`
- Installed the rebuilt `golem` binary and restarted the existing demo server
  without resetting `./data`; it resolved router `19881`, custom-request
  `19006`, and MCP `19007` from the demo manifest.
- Queried a real 200-agent batch with a continuation cursor, then smoke-tested
  the installed TUI's column chooser, AgentID details formatting, horizontal
  panning, details focus/scroll/resize, auto-refresh indicator, and loaded-row
  Find scope/count against that dataset.

## 2026-09-23

- Chose an Ops-first production rebuild rather than restyling the legacy
  Home/Dev/Ops workbench. Ops is the only initial workspace and keeps
  `[Ops]` as the single workspace-selector row.
- Defined Agents as the initial subject. Overview and Metrics are the first
  production views; Activity remains one future view with interpreted Timeline
  and exact Journal modes.
- Chose live typed agent data plus truthful metrics availability states as the
  first implementation slice. The metrics receiver, store, and query backend
  remain deliberately out of scope until their contracts are locked.
- Recorded the local observability direction for GOL-162: a metrics-only OTLP
  receiver owned by local `golem`, a bounded dedicated `observability.db`, and
  an effective deployment overlay applied only during normal deploy/update.
- Preserved user-configured external exporters, prohibited startup revisions
  and manifest edits, and retained native APIs for logs and oplogs.
- Defined explicit cross-agent selection, visible source limits, authoritative
  per-agent oplog ordering, and timestamp merging as presentation rather than
  global causality.
- Rebuilt the production shell around Ops and Agents, promoting the accepted
  frame, pane, table, selection, scrollbar, and compact-layout patterns.
- Added the live Agents Overview with filtering, explicit multi-agent
  inclusion, horizontal column panning, a transactional column chooser,
  optional details, typed refresh, and stale-data preservation on errors.
- Split collection filtering into an exact server-side Dataset selector and a
  field-scoped fuzzy Find over loaded rows. The UI now keeps dataset, Find
  scope/query, loaded match count, loaded row count, and cursor availability
  explicit.
- Added bounded cursor continuation at 200 rows per component, automatic
  continuation near the loaded end, explicit Load More, and bulk All/None over
  loaded Find matches only. No page number or global total is synthesized.
- Made refresh state visible in the pane header, including auto-refresh,
  refreshing, and loading-more states.
- Made Agent details focusable, mouse/keyboard scrollable, and mouse/keyboard
  resizable. Details reuse the CLI AgentID formatter, semantic status colors,
  deployed agent-type metadata, instance metadata, and colored JSON.
- Added reusable collection query bars, pane-header status, semantic table
  cells, scrollable documents, colored JSON, and resizable split state.
- Added a reachable Metrics view that reports current availability without
  inventing series. The receiver, store, query API, and deployment adaptation
  remain future observability work.
- Removed the legacy Home, Dev, server, output, REPL, and nested inspect routes
  from the visible production shell while retaining their dormant foundations
  for later rebuilt workflows.
- Added deterministic preview stories for the production Ops views and the
  future Activity Timeline and Journal contract. The production rebuild is now
  awaiting visual acceptance.

## 2026-09-21

- Accepted odd/even row surfaces as the main-pane table decoration, retaining
  cell rules for compact non-wrapping tables.
- Added a restrained amber tint to selected odd/even row surfaces and used the
  stronger accent for the full-height `▌` rail without changing selected text.
- Closed the pane-table foundation review and advanced the active preview focus
  to normal and 50×16 compact shell layouts.
- Accepted the compact shell below 72 columns or 20 rows: the header uses the
  existing `app/environment/server` path grammar, workspace navigation retains
  numbered Home/Dev/Ops labels while they fit, and the footer preserves only
  workspace navigation plus Commands/Help/Quit.
- Completed the generic visual-foundation review and advanced planning to a
  user-goal inventory for the minimum developer and DevOps workflows.

## 2026-09-02

- Added semantic content primitives for section headings, aligned fields,
  non-color status markers, loading/error/unavailable/empty/info notices,
  selectable fixed-column tables, and bounded output rows.
- Rebuilt the content design-lab focus from those components and added
  single-pane, split-pane, scrolling, and long/narrow stories. Production
  rendering remains unchanged.
- Added focused component invariants and all-story wide, short, narrow, and
  extreme non-zero render coverage.
- Removed the legacy `> Content hierarchy  tab focus` row from content stories.
  Content panes now use only the locked joined boundary title; side panes do not
  add local focus rails, headers, or shortcut hints.
- Consolidated one-or-more-pane horizontal geometry in `PaneLayout`, making
  headers, body dividers, and footer junctions share the same weighted columns.
  Removed the duplicate secondary content rail and added right-edge, shared
  divider, and three-pane structural invariants.
- Moved the outer left body spine into `PaneLayout` and render it after content,
  preventing row backgrounds and padding from punching gaps through the line.
- Completed the shared notice vocabulary with active, success, and warning
  states, retaining `!` as the non-color warning identity alongside explicit
  labels for every state.
- Assigned active notices to the positive green semantic token alongside
  running and success, rather than the orange focus/accent token.
- Right-aligned contextual quick-hint groups while keeping their left structural
  spine fixed; modal shortcuts remain centered and workspace navigation is
  unchanged.
- Replaced the three statically reserved shortcut rows with one global row and
  width-aware packing of merged workspace/pane hints into one or, only when
  necessary, two contextual rows. Footer height now follows the packed result.
- Removed stale right padding from split-pane output width, so overflowing
  output places its ellipsis in the terminal-facing final cell.

- Extracted named preview components for the locked visual foundation: context
  headers, pane boundaries, workspace selectors, shortcut rows, neutral
  scrollbars, popup frames, search inputs, selectable rows, and decision-table
  rows. Frame Base stories use these representations while Production remains
  unchanged.
- Made programmatic representation and focused verification part of accepting
  every future design decision, while allowing the same representation to be
  revised when a decision is reopened.

## 2026-09-01

- Selected Hint None: pane headers now reserve their joined labels for identity,
  while shortcut guidance belongs in footer or modal control surfaces.
- Recorded the durable active-square/idle-round title grammar and the rule that
  every workspace is a one-or-more-pane layout.
- Added Flat, Joined, and Nav Last unified-footer preview candidates with
  persistent global/workspace/pane rows, transient leader coverage, and no
  generic `More` entry.
- Added compact priority fallback and single-pane coverage without changing the
  Production renderer or keymap.
- Selected Unified Joined as the bottom control-zone structure and promoted it
  into Frame Base.
- Added Soft, Deep, and Closed Spine mutations that share a darker background
  with the context header and tone down the joined footer boundary.
- Selected Closed Spine while retaining the original footer background and
  sharing it with the context header.
- Made all structural decorators neutral and muted footer navigation and key
  labels, leaving the active pane title as the only orange focus signal.
- Advanced the focused comparison to Labels, Dividers, Path, and Chips for
  context-header metadata.
- Selected Dividers for context metadata and promoted it into Frame Base.
- Shifted footer key labels from gray to subdued amber, still darker than the
  active-pane selection accent.
- Added Padded, Compact, and Regular plain-chrome candidates that remove special
  header/footer backgrounds and render GOLEM as orange text.
- Completed the shell's joined left spine with `┌` on the identity row as the
  counterpart to the selected footer `└`.
- Applied the subdued shortcut amber to the `1`, `2`, and `3` workspace keys in
  the joined footer navigation row.
- Selected Plain Padded, removed special header/footer backgrounds from Frame
  Base, and retained GOLEM as padded bold orange text.
- Added Dot, Slash, and Space candidates to replace unconnected vertical
  context dividers.
- Selected Dot for context metadata and added the same neutral separator between
  GOLEM and the first application pair.
- Added an Angle Values mutation with gray `<` and `>` delimiters around bright
  app, environment, and server values while retaining the dot rhythm.
- Expanded value framing into four focused spacing candidates: tight, outer,
  inner, and full; Frame Base remains the accepted plain-value reference.

## 2026-08-31

- Promoted Joined Bracket into Frame Base after pane-header review.
- Narrowed the active focus to three prefix-free treatments: active title,
  connected rule, and filled bracket label.
- Promoted Active Title into Frame Base and narrowed the next comparison to
  round idle, plain idle, and double active delimiter shapes.
- Promoted Round Idle into Frame Base and narrowed the next comparison to hints
  inside the title, outside it, on the rule, or omitted in favor of the footer.
- Queued matching shell header and footer chrome after the pane focus decision.

## 2026-08-27

Browser preview typography:

- Split HTML buffer export into aligned background and foreground layers so
  later row backgrounds cannot cover glyph descenders while terminal rails keep
  a one-em, gapless row pitch.
- Replaced platform and generated-CSS font choices with version-pinned Fira Code
  and commit-pinned Iosevka Term WOFF2 files.
- Added browser-side font loading, monospace/glyph-width checks, vertical rail
  edge checks, and blocking failure behavior instead of silent fallback.
- Added a calibration sample using descenders and the TUI's actual decorator
  glyphs, while keeping font and size selections linkable.
- Confirmed through the forwarded macOS browser that Fira Code and Iosevka Term
  both pass qualification and preserve readable text with joined decorations.

TUI generic design lab:

- Replaced the workflow-first preview catalog with six focused real-renderer
  cases covering shell chrome, content density and states, mixed splits and
  focus, leader shortcuts, search, and confirmation.
- Narrowed the color-oriented experiments and temporary Frame branches to one
  actively iterated Frame Base beside Production.
- Removed workflow fixture construction from the preview path while retaining
  the existing production semantic tests and explicit production-wrapper render
  equivalence.
- Added semantic design-lab checks for progressive shortcut disclosure,
  non-color focus markers, modal-local controls, status vocabulary, route and
  revision responses, and separation between Production and Frame Base.
- Reopened product workspace and flow decisions for a goal-led review after the
  generic foundation is accepted; production behavior remains unchanged.
- Retired the temporary Header, Popup, and Quiet branches after they clarified
  the individual choices; subsequent changes land directly on Frame Base.
- Made the interactive terminal design lab watched as well. It catches watcher
  termination, leaves blocking input within 100 ms, and restores raw mode,
  cursor, and alternate-screen state before cargo-watch rebuilds and restarts it.
- Replaced Frame Base's `├` focus rail with `│` in panel headers and
  unframed overlays; focus remains visible through the adjacent marker and style.
- Removed the remaining left gutter from Frame Base panel titles and view
  content, matching the compact rail spacing already used by its main headers.
- Added isolated Header and Panes experiment families: three uniform metadata
  treatments and three single-owner boundary strategies, with browser filtering
  and terminal cycling across the preview-only candidates.
- Replaced parallel experiment families with one four-card current focus. Pane
  boundaries are active, header metadata is queued, inactive header variants
  are removed, and Production/coverage controls are collapsed as secondary tools.
- Selected Shared boundary ownership and folded it into Frame Base, then narrowed
  the active mutations to band, chip, and underline pane-header treatments.
- Increased Band and Chip contrast through a dedicated pane-header surface
  without changing overlays, the bare/underline options, or Production.
- Retired the still-subtle background treatments in favor of joined, joined
  accent, and joined bracket headers that occupy the actual split boundary rows;
  also gave Frame previews a more visible dedicated footer surface.
- Validation: normal and feature-gated `golem-cli` checks and all 14 focused
  preview tests pass. Manual direction comparison remains the next acceptance
  step.

## 2026-08-06

TUI DX/UX review workflow:

- Made browser preview cases independently addressable, including compact and
  wide cases that reuse an underlying deterministic story fixture.
- Added Production-first Review, side-by-side Compare, and single-variant
  Coverage views with URL-backed selection and copied review links.
- Made browser typography terminal-oriented with a Fira Code default, disabled
  ligatures, remotely loaded open-source webfonts, a visible glyph sample,
  selectable monospace fonts, and link-persisted font size. Remote loading keeps
  previews consistent when the Linux server is viewed through a Mac port
  forward.
- Added Noto Sans Symbols 2 as the deterministic Braille fallback and exposed
  whether the selected primary webfont finished loading.
- Removed the large Braille Golem logo from the Home background because its
  platform-dependent glyph rendering obscured preview review. Logo placement
  and representation remain a future DX/UX decision.
- Reduced watched-gallery revision polling from 700 ms to 2 seconds per tab.
- Added `dx-ux-review.md` as the long-lived operational ledger for
  story-by-story feedback and explicitly approved themed implementation batches.
- Defined synchronization rules so durable decisions, executable tasks,
  validation history, and active feedback stay in their existing authoritative
  documents.

## 2026-07-02

TUI visual preview workflow:

- Added a dependency-free static HTML preview gallery under `.tui-plan/design-previews/`.
- The gallery compares four visual-language directions for headers, split affordances, color weight, density, server drawer treatment, palette styling, and Ops contrast.
- The preview is explicitly non-authoritative; accepted directions must be copied into `.tui-plan/ui-system.md`, then implemented in Ratatui and validated with render tests.

## 2026-07-01

Pointer-native layout and global server drawer:

- Added `tui::layout` as the first dedicated layout/hit-test module.
- Render now computes a named `LayoutSnapshot` and stores it on `TuiApp` for mouse handling.
- Mouse wheel routing now targets the panel under the pointer rather than the keyboard-focused panel.
- Pointer clicks now switch workspace tabs, focus Dev panels, select Ops agent rows, focus inspect panes, and activate context picker/confirmation regions.
- Added session-only split dragging for Dev primary/secondary splits and the global server drawer, with ratio clamps for constrained terminals.
- Added Dev layout presets for right, left, top, and bottom panel placement.
- Added `ctrl+x l` / palette action for cycling Dev layout presets.
- Added a global local-server drawer, closed by default and available from Home, Dev, and Ops.
- Added `ctrl+x v` / palette action for toggling the server drawer.
- When the drawer is open, Dev skips the large Server panel so server logs have one primary surface.
- Added focused layout and pointer behavior tests for preset geometry, drawer splitting, hit testing, region scrolling, clicks, modal actions, and split dragging.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli --lib -- tui:: --report-time`

Context value separator focus narrowed.

Current status:

- Retired the four angle-bracket spacing mutations.
- Kept the selected dot separators between GOLEM and context groups.
- Reduced the active comparison to space (`app value`), colon (`app:value`),
  and equals (`app=value`), with punctuation using the muted label color.
- Switched the focused case to `split-focus` so the selected space form can be
  reviewed against joined multi-pane geometry before promotion.
- Recorded a later interaction requirement: contextual shortcut hints will be
  toggleable, but the primary `1` / `2` / `3` workspace selector stays visible.
- Locked the whitespace context grammar (`app value`) and retired the colon and
  equals mutations.
- Started a four-card split-pane focus comparing open, capped, square-turn, and
  round-turn terminal-facing pane-title endings.
- Selected the square turn, continued the terminal-facing rail to a `┘` footer
  connection, and conditionally joined unobstructed split rails with `┴`.
- Revised the right edge to a title-only `┐` cap with no vertical rail or footer
  corner, reclaiming that column while retaining the conditional middle `┴`.
- Added isolated `┤` endings for stacked pane titles and `┘` for the last footer
  row, and made the focused browser view show single- and multi-panel cases
  together.
- Moved the right-side `┘` from the final shortcut row to the persistent
  `[1, 2, 3]` workspace-selector row.
- Added single- and multi-panel scrollbar cases using Ratatui's scrollbar widget,
  plus edge-reaching content in the non-scrollbar cases for direct comparison.
- Muted scrollbar arrows and thumbs to gray and made the track a fainter gray so
  scrolling chrome does not compete with pane focus.
- Locked all vertical scrollbars to the trailing-right pane cell. Each visible
  scrollbar now reserves exactly one content column, and split and stacked
  stories use the same pane region for content, boundaries, scrollbar slots,
  resize-divider metadata, and hit geometry.
- Added testable terminal resize-event handling that propagates the new Ratatui
  viewport dimensions and requests a redraw.
- Accepted the general Content Primitives batch and split richer main-pane data
  tables from the compact popup and summary row grammar.
- Added preview-only pane-table columns and state with required/optional
  visibility, explicit widths, per-column ellipsis or selected-only wrapping,
  a frozen marker, restrained multi-line selection, and horizontal panning.
- Added deterministic table-decoration, long/panned, selected-details, and
  column-chooser stories. The chooser applies visibility changes
  transactionally; table and details panes retain independent geometry and
  scrollbars.
- Replaced the main-pane table's diamond selection marker with a solid `▌` rail
  repeated across the selected record's full wrapped height. Popup and search
  selection markers remain unchanged.
- Removed space padding around table cell rules, moved the Columns chooser to
  the compact popup-table selection grammar with an `↑/↓ Navigate` hint, and
  restricted multi-line rows to odd/even tables with four explicit row
  surfaces for odd/even × selected/unselected.
- Made shared fitting and pane-table wrapping use terminal display width so
  wide, emoji, and combining characters preserve cell geometry, and made table
  virtual-width accumulation saturating.
- Extended pane hit testing to nested horizontal resize-divider metadata, with
  divider precedence over scrollbar and body targets.
- Accepted the pane-boundary and scrollbar treatment, then moved the active
  review to search, confirmation, error, and nested-modal baseline cases.
- Replaced the overlay side-rail treatment with complete outer borders, inset
  content, and a more distinct shared background for border and interior cells.
- Rejected half-cell block borders as too heavy. Restored narrow box-drawing
  borders while keeping their cell backgrounds on the underlying surface, so
  the distinct popup background begins inward and does not halo outside the box.
- Chose the narrow connected panel-style border compromise: border cells use the
  popup background, accepting its outer half-cell halo to avoid a double or
  uneven inner boundary.
- Added optional `─[ Title ]` overlay titles with neutral brackets and contextual
  title colors, plus a darker search-input row with a muted `>` prompt.
- Replaced bracketed overlay titles with border-background labels using exactly
  one cell of left and right padding.
- Lightened warning and error title-label colors independently from body status
  colors, improving their separation from the gray border background.
- Shifted the error title token from pale pink to a clearer warm red while
  retaining sufficient contrast on the gray title background.
- Added a dark overlay-title comparison using dark gray, amber, and red text
  beside the existing light title set, without changing popup geometry.
- Rejected the dark title set, restored the accepted white and pale amber, and
  narrowed review to warm, signal, and vermilion error-title reds.
- Rejected per-status title colors and standardized all overlay titles on the
  GOLEM yellow accent with neutral `[ Title ]` brackets.
- Made the search field lighter than its popup and centered it with one cell of
  popup-surface margin on every side.
- Replaced search `>` markers with a muted `›` prompt and `▌` selection marker,
  added darker green query text, centered popup content, and aligned popup
  shortcut styling with the main footer.
- Centralized one-cell popup content padding on every side and used blank rows,
  rather than input-specific margins, between search, results, and shortcuts.
- Standardized search entries as equal-width rows with aligned labels and a
  consistent near-white selection background spanning the full row.
- Darkened the selected-row background and inserted a blank row after the main
  `!` / `×` message in confirmation and nested-modal content.
- Converted confirmation job lists into a centered, fixed-width `Job` / `State`
  table with left-aligned columns.
- Darkened the fixed-width search selection background one additional step while
  preserving its dark text and geometry.
- Restored the selected `[ Overview ]` pane header beneath GOLEM on every overlay
  case, replacing the disconnected generic separator with the joined `├` row.

Selected Context UX foundation:

- Added a global context picker opened with `ctrl+x e` and through the palette.
- Context candidates are built from the launch context, manifest environments, explicit local/cloud selections, and configured profiles.
- Switching builds a new immutable `Context` snapshot from cloned CLI flags and installs it into the TUI context executor with a new generation id.
- Context switching is blocked while context-bound work is active; confirmation/stop-and-restart flows are deferred.
- Nested dev/interactive commands inherit the selected context arguments, while direct Agents refresh uses the selected executor context.
- Command, server, REPL, agent inspect, and agent refresh status surfaces show their launch context.
- Local-server actions are disabled, and direct shortcuts are guarded, when the selected context is not local-server backed.

Selected Context UX scoped selector:

- Replaced the flat context candidate list with scoped context targets: manifest app contexts, server targets, and server app-environment targets.
- The context picker now uses a two-step flow. Manifest app contexts switch directly. Server targets first list visible app environments through a direct typed environment handler call, then the selected app/environment becomes the Ops scope.
- Server target discovery now includes manifest environment servers, built-in local/cloud servers, configured profiles, and launch-selector-implied targets while preserving source/auth variants as separate rows.
- Agents refresh carries the selected typed `EnvironmentReference` into the direct worker list request, so server app-environment scopes no longer rely on nested CLI parsing.
- Dev actions are enabled for manifest app contexts and disabled for Ops-only server app-environment contexts. Local server actions still additionally require a local-server-backed context.
- Agent inspect oplog/stream remain nested CLI transitional Ops debt until the direct streaming provider milestone.

Selected Context UX architecture correction:

- The first picker foundation is intentionally not the final context model.
- The design now separates manifest app contexts, server targets, and server app-environment contexts.
- Manifest app contexts enable Dev + Ops. Server app-environment contexts are Ops-only unless they match the current manifest app/environment.
- Server targets must be derived from manifest environment servers, built-in local/cloud servers, configured profiles, and all launch selector inputs, not only `--local` and `--cloud`.
- Overlapping server endpoints must preserve source/auth variants instead of being silently collapsed.
- The current `GolemCliGlobalFlags`-based candidate representation is documented as a temporary implementation gap; future work needs a TUI target descriptor that can materialize contexts or direct clients.

## 2026-06-08

Initial planning context captured.

Decisions recorded:

- Implement the TUI in `cli/golem-cli` so both `golem tui` and `golem-cli tui` work.
- Prefer nested CLI sessions early where they preserve behavior and enable multi-environment workflows.
- Choose direct API integration case by case.
- Treat local, cloud, and custom targets as environment selections rather than separate UI modes.
- Use `.tui-plan` markdown files to track design, tasks, and progress.
- Keep implementation simple and optimized for human reviewability.
- Avoid needless tests and abstractions.
- Build agentic coding loops from the beginning with deterministic render/frame inspection and small targeted checks.

Repository observations:

- `golem-cli` already depends on `crossterm`, `portable-pty`, `expectrl`, `fuzzy-matcher`, and `goldenfile`.
- `ratatui` was not in workspace dependencies before this scaffold.
- Existing REPL implementation goes through `ReplHandler` and `TypeScriptRepl`, with PTY supervision for interactive mode.
- Existing CLI interactive tests use `expectrl` and can inform TUI PTY tests.
- Existing command metadata collection can be reused for command discovery.

Current status:

- Planning documents have been created.
- TUI scaffolding has started with dependency wiring, command dispatch, a minimal terminal runtime, and a dashboard render test.
- Added `ratatui 0.30.1` as a workspace dependency and wired it into `golem-cli`.
- Added the top-level `tui` command for both `golem-cli tui` and `golem tui` through existing command dispatch.
- Excluded `tui` from REPL command metadata because it is an interactive top-level mode, not a REPL command candidate.
- Added a minimal TUI runtime using `ratatui` plus `crossterm`, with raw mode, alternate screen, cursor hide/show, and cleanup through a terminal guard.
- Switched the scaffold runtime from fixed polling to blocking terminal events; redraws now happen after keypresses and resizes.
- Added an initial dashboard render with selected application/environment/server/config context.
- Added a focused `test-r` render test that captures the `TestBackend` buffer as readable text for agentic inspection.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli renders_dashboard_frame`
- RustRover build check for the touched TUI and command wiring files

Navigation and palette scaffold:

- Added static views: Dashboard, Environments, Components, Agents, and Output.
- Added tab rendering and view switching with `]`, `[`, `Tab`, `Shift-Tab`, and `1` through `5`.
- Added a command palette opened with `Ctrl-P` or `:`.
- Added fuzzy filtering with `fuzzy-matcher` over built-in TUI actions.
- Added built-in palette actions for view switching and quitting.
- Added keyboard routing so palette input is handled separately while the palette is open.
- Added focused tests for active tab rendering, tab switching, numeric jumps, palette opening, palette filtering, and palette action execution.
- Workflow decision: after TUI implementation changes, prepare the matching manual playground by reinstalling `golem` for the active checkout. For this checkout, use `CARGO_INSTALL_ROOT=/Users/noise64/.cargo-alt-02 cargo make install-golem-dev-release`, then test from `/Users/noise64/workspace/golem-demo/golem-02` with `/Users/noise64/.cargo-alt-02/bin` first in `PATH`.
- Fixed tab label jitter by rendering inactive tabs with the same width as active bracketed tabs.
- Added `TuiMode` for normal, palette, and help modal routing.
- Added `?` help overlay with current global and palette shortcuts.
- Added `Show Help` as a built-in command palette action.
- Added focused tests for opening help, closing help, and opening help from the palette.

Build/deploy nested CLI feature:

- Added a channel-driven TUI event loop so terminal input and nested command output are both event sources.
- Added a PTY-backed nested CLI runner for finite commands.
- Added build/deploy shortcuts: `b` and `d`.
- Added `--yes` and `--reset` toggles: `y` and `r`.
- Added CommandInteraction mode for build/deploy prompts and finite command input.
- Added Ctrl-C/Esc cancellation with second press force-kill escalation.
- Added Output view rendering for command status, flags, command line, and scrollable output.
- Added auto-following output with PageUp/PageDown/Home/End scroll controls.
- Added ANSI color preservation with `ansi-to-tui`, with plain fallback on parse failure.
- Added command palette actions for Build, Deploy, Toggle Yes, and Toggle Reset.
- Added focused tests for toggles, command state, output follow/scroll behavior, and cancel escalation.
- Added native cursor placement for command interaction input, using the terminal cursor rather than drawing a fake cursor.
- Made `yes` and `reset` flag state visible in the global footer.
- Compacted the Output command summary to a single status line.
- Enabled mouse capture and mouse-wheel scrolling in the Output view.
- Added a vertical scrollbar for command output.
- Added focused tests for footer flags, compact status, command-interaction cursor placement, arrow-key scrolling, and mouse scrolling.
- Added `clean` as a PTY-backed nested CLI command with `c` shortcut and palette action.
- Styled footer and compact Output status hints so shortcut letters and enabled flags stand out.
- Fixed top-of-output scrolling so the viewport stays filled instead of collapsing to a single line plus empty space.
- Fixed scrollbar positioning so the thumb reaches the bottom when viewing the latest output.
- Added focused tests for clean shortcut, clean palette action, and top-scroll viewport filling.
- Added a focused test for output scrollbar top/bottom position mapping.
- Stabilized the Output view layout by always reserving the command input row.
- Removed active-tab brackets and switched active tab indication to styling only, avoiding tab label width changes.
- Reworked the footer into fixed-width command and flag segments to avoid reflow when toggling `yes` or `reset`.
- Reworked compact command status into fixed-width cells for command kind, status, flags, command, and hint.
- Kept the main boxes for this pass; visual simplification can continue after manual review.
- Added focused tests for fixed-width labels and stable Output input-row reservation.
- Added a per-command spinner event source that runs only while a nested command is active.
- Rendered the spinner in the compact command status row for running/cancelling commands.
- Stopped spinner ticks when commands finish, fail to start, are killed, or are cleaned up on TUI exit.
- Added a focused spinner tick test.
- Modernized the main TUI chrome by removing boxes from the header, non-output body, and Output sections while keeping modal boxes.
- Added a subtle horizontal separator under the tab row.
- Added subtle background colors for header, tabs, body surface, command status, and footer.
- Added section prefix glyphs (`┃`, `│`) to restore visual structure without returning to heavy boxed layouts.
- Standardized a left `┃` rail across header, tabs, separator bar, surfaces, command status, output, input, and footer.
- Replaced the horizontal dash separator with a full-width background bar row and a left rail.
- Changed the spinner first frame from `-` to `|` to better match the rail-based visual language.
- Added a Server tab backed by a separate PTY `golem server run` job.
- Added server log capture with independent scrollback and autofollow.
- Added server controls: `s` start/stop, `R` restart, `x` toggle clean, `C` clean restart.
- Added server palette actions for start, stop, restart, clean restart, toggle clean, and go to Server.
- Kept server logs separate from finite build/deploy/clean command output so both can coexist.
- Added focused tests for server initial rendering, clean toggles, start/stop/restart state, server log separation, and mouse scrolling.
- Made the command palette width content-driven, with terminal-margin clamping for narrow screens.
- Kept command palette width stable while filtering by sizing it from the full action catalog, not the filtered result set.
- Kept command palette height stable while filtering by sizing it from the maximum visible action count, not the filtered result set.
- Removed placeholder Environments and Components tabs for now.
- Replaced the Agents placeholder with a fuzzy-filtered agent list backed by `golem agent list --format json --mode <mode>`.
- Added agent mode cycle: durable, ephemeral, all.
- Added selected-agent details side panel with raw JSON fallback.
- Added manual agent refresh (`u`) and auto-refresh toggle (`a`).
- Added Agent filter mode entered with `/`, with editable query and Up/Down selection.
- Added agent palette actions for refresh, auto-refresh, mode cycle, and details toggle.
- Added robust JSON parsing for both array and `{ values: [...] }` agent list shapes.
- Added focused tests for tab removal, agent filtering/selection, mode cycling, details panel, refresh result handling, and JSON parsing.
- Added reusable PTY input encoder for future REPL mode and reused it for finite command interaction.
- Added PTY input encoding tests for printable, control, alt, and navigation keys.
- Added a `vt100`-backed terminal screen wrapper for future REPL rendering.
- Terminal screen wrapper can feed PTY bytes, resize, expose cursor position, render Ratatui lines, and retain plain text for assertions.
- Added terminal screen tests for plain text, cursor movement, cursor position, SGR styles, and resizing.
- Hid the command input row when no command is running or when the current command was started with `yes:on`.
- Preserved the command input row for the lifetime of commands started with `yes:off`, even if the future-run `yes` toggle changes during the run.
- Added focused tests for hidden/visible command input row behavior.
- Added a REPL tab backed by nested PTY `golem repl`.
- Added REPL-specific TUI events for PTY output, output closure, and process exit.
- Added `ReplState` and `ReplRun` with a `vt100` `TerminalScreen` instead of line-oriented output buffering.
- Added REPL view rendering with a compact status row, embedded terminal screen, and native cursor placement.
- Added REPL focus mode that sends keys directly to the REPL.
- Added `Ctrl-X` REPL leader commands: `q` leave focus, `k` stop, `p` palette, `?` help, `r` restart.
- Added `l` / Enter start-or-focus behavior and REPL palette actions for start/focus/leave/stop.
- Added focused tests for REPL start/rendering, terminal-screen output, cursor placement, and leader behavior.
- Researched non-TUI CLI styling and decorator conventions: green action words, yellow warnings, red errors, bold highlights, `-` bullets, UTF-8/ASCII comfy-table presets, `╔═`/`║`/`╚═` decorated help blocks, and `│` child-process output gutters.
- Researched opencode theme/keybinding conventions: semantic theme tokens, truecolor expectation, system/terminal theme mode, `none` terminal defaults, and `Ctrl-X` leader key use.
- Researched golem.cloud visual tokens: dark neutral surfaces, amber primary accent, warm orange marker, muted grey text scale, and dark code-block surfaces.
- Added an internal Golem-branded dark TUI theme token set; no custom/system theme support yet.
- Switched TUI chrome from cyan-led styling to Golem amber active/focus styling.
- Kept nested CLI output and REPL screen ANSI-native; the Golem theme only styles surrounding chrome, gutters, status rows, palette, help, and selection surfaces.
- Added tab numbering in the tab strip.
- Changed the direct REPL shortcut from `l` to `r`.
- Added a general `Ctrl-X` leader mode for settings and secondary actions.
- Moved `--yes`, `--reset`, server clean, server restart/clean-restart, and agent settings toggles behind `Ctrl-X` leader shortcuts.
- Added unified shortcut styling in footer, palette rows, and leader hints.
- Added `Restart REPL` to the command palette and bound focused REPL restart to `Ctrl-X R`.
- Added focused tests for numbered tabs and leader shortcut hint rendering.
- Polished the Golem-branded theme after manual review: removed redundant content/footer rails, made dashboard surface rails consistently gray, changed the Agents detail divider to gray, normalized output line backgrounds to the TUI surface, added segmented warm header backgrounds, and added spaces after header labels such as `app: `.
- Follow-up polish: restored gray content rails consistently on dashboard/content/output lines so non-empty rows no longer erase the surface rail while blank rows keep it.
- Fixed the dashboard rail rendering at the root cause: body content now reserves the first columns and the gray surface rail is drawn after body content, with a regression test asserting every dashboard body row keeps the rail. The header is now a full amber background with darker warm segment backgrounds for `app: `, `env: `, and `server: `.
- Header polish: restored the first-line `┃` separator, removed the multi-background gradient, and made the full header row amber with separate label/value text colors.
- Palette polish: added an inner `┃` side decoration to command palette content.
- Server UX polish: replaced separate `Start Server` and `Stop Server` palette entries with a single `Start/Stop Server` action using `s`, made `s` global so it jumps to Server and toggles the server, and added Server-tab `Enter` to toggle server like REPL-tab `Enter` starts/focuses REPL.
- Tab polish: tab numbers now use the same highlighted shortcut styling as footer/palette shortcuts.
- Added live tab indicators on the tab row for running Output commands, Server, and REPL sessions.
- Replaced the hand-written Braille dashboard mark with faint centered Braille art generated from the in-repo `website/src/assets/logo/golem-horizontal-white.png` logo asset.
- Changed tab running indicators to always render for Output, Server, and REPL: gray `○` when idle and green `●` when running.
- Widened the generated Braille logo by changing its aspect ratio, darkened it to a more background-like gray, and made dashboard text rows paint their full width so the logo does not bleed through after foreground text.
- Simplified the command palette by removing the old box border and changing the side rail to a full-height yellow rail for the whole palette subwindow.
- Added an Agents inspect subview within the Agents tab. Pressing Enter on a selected agent opens a split view with `golem agent oplog <agent>` on the left and `golem agent stream <agent>` on the right.
- Agent inspect mode uses independent PTY-backed nested CLI targets and output buffers for oplog and stream.
- Left/Right switches focused pane; Up/Down, PageUp/PageDown, Home/End scroll the focused pane; Esc kills both inspect jobs and returns to the agent list.
- Added focused tests for opening inspect mode, pane focus switching, focused-pane scrolling, Esc returning to the list, split-view rendering, and separate event routing for oplog/stream buffers.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli tui::app::tests`
- `cargo test -p golem-cli -- tui::app::tests tui::terminal_screen::tests`
- `cargo test -p golem-cli -- tui::`
- `cargo check -p golem-cli`
- `CARGO_INSTALL_ROOT=/Users/noise64/.cargo-alt-02 cargo make install-golem-dev-release`
- `PATH="/Users/noise64/.cargo-alt-02/bin:$PATH" golem tui --help` from `/Users/noise64/workspace/golem-demo/golem-02/test-app`
- RustRover build check for touched TUI files after theme/keymap pass
- RustRover build check for `cli/golem-cli/src/tui/app.rs`
- RustRover build check for `cli/golem-cli/src/tui/mod.rs` and `cli/golem-cli/src/tui/nested_cli.rs`
- RustRover build check for `cli/golem-cli/src/tui/terminal.rs`
- RustRover build check for `cli/golem-cli/src/tui/app.rs` after clean/scroll polish

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli tui::app::tests`
- RustRover build check for `cli/golem-cli/src/tui/app.rs`

## 2026-06-29

Planning reset and first foundation goal implemented.

Decisions recorded:

- Future TUI work is organized around six arcs: action/help rules, test driver/regression harness, scoped logging/context execution, context/environment model, dev/ops workspace structure, and local observability for issue #3456.
- The immediate implementation goal is "Action Rules And TUI Test Driver Foundation".
- The TUI should derive visible command names and shortcuts from action metadata where practical.
- Raw input controls such as text entry and scrolling can remain explicit help controls until they become actions.

Current status:

- Reworked `.tui-plan` documents around the current implementation state and foundation-first arcs.
- Extended `TuiAction` with stable IDs, categories, scopes, and palette visibility.
- Made palette filtering respect palette visibility and include action category text in fuzzy matching.
- Derived footer command labels/shortcuts and leader workflow shortcuts from the action registry.
- Replaced the static help body with generated sections backed by registered actions plus explicit raw input controls.
- Made Agent inspect help reachable with `?`.
- Added a test-only `TuiTestDriver` with key input, frame capture, visible/absent text assertions, and failure dumps.
- Added high-level driver scenarios for global action help, leader action help, palette/action-registry consistency, Agent inspect scoped help, and REPL leader help.
- The new driver initially caught that full-help assertions were invalid at 24 rows because the help modal clipped lower sections; the driver now uses a taller frame for full-help scenarios while existing 100x24 tests remain unchanged.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli --lib -- tui::`

Direct Agents Provider Foundation was explored, then reverted.

Async TUI blocking-boundary cleanup:

- Replaced blocking terminal event reading with `crossterm::event::EventStream`.
- Kept `portable-pty` as the nested interactive backend, but isolated remaining blocking calls behind named PTY adapter helpers.
- Changed nested CLI runtime control operations so key handling, resize handling, and stop/kill actions queue bounded control messages instead of calling PTY write/resize/kill methods inline.
- Documented that `spawn_blocking` is allowed only for explicit blocking-library adapters, not normal TUI orchestration or direct ops/provider work.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli --lib -- tui:: --report-time`

Direct typed Agents refresh:

- Added a neutral `AgentListRequest` and `WorkerCommandHandler::list_agent_metadata` that returns `AgentsMetadataResponseView`.
- Kept CLI `agent list` rendering/logging at the CLI edge while sharing the typed listing helper.
- Changed the TUI Agents refresh to run through `TuiContextExecutor`, carrying generation plus launch context identity into `AgentRefreshFinished`.
- Mapped `AgentsMetadataResponseView` into `AgentListItem` directly and kept details backed by serialized typed metadata.
- Removed production JSON parsing and nested `golem agent list --format json` subprocess refresh from the Agents view.
- Made the context-executor reuse-id test order-independent because concurrent requests can complete in either order.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli --lib -- tui:: --report-time`
- `cargo test -p golem-cli --lib -- command_handler::worker --report-time`

Decisions recorded:

- Ops/resource exploration should use direct typed calls by default.
- Nested CLI remains appropriate for dev/interactive workflows where PTY behavior matters.
- Remaining ops nested CLI uses, including Agent inspect oplog/stream, are transitional debt until direct streaming providers exist.
- A central `TuiOpsProvider`/`TuiDataProvider` is the wrong shape if it absorbs every view's request and event mapping.
- The next direct-call foundation should be a smaller context executor: selected context ownership, launch context cloning, target generation, background async execution, logging scope, and completion delivery.
- Views should keep request-specific logic. For Agents, the view/app state should own mode selection, local mapping to rows, stale result handling, filtering, details, and events.
- Command handlers should not grow TUI-specific methods such as `list_agents_for_tui`. Extract or expose neutral data-returning helpers and keep CLI rendering at the CLI edge.
- Proper logging context is required before broad direct handler use. Thread-local logging alone is insufficient because existing `LogIndent` scopes commonly cross `.await` and TUI work may run concurrently.
- Logging context should be explicit and async-aware: a scoped `LogContext` owns output mode, indentation, buffers, and captured lines; lookup can use tracing span extensions and Tokio task-local scope before falling back to global CLI logging.
- Future environment switching should replace selected `Arc<Context>` values instead of mutating a live `Context`.

Current status:

- Reverted the premature provider implementation, including the central provider module, `WorkerCommandHandler::list_agents_for_tui`, and typed Agents refresh event wiring.
- Updated `.tui-plan` to make scoped logging and a context executor the next foundation before moving Agents list to direct typed calls.
- Kept the hard architecture rule: dev/interactive workflows may continue using nested CLI/PTTY, while ops/resource refresh-heavy views should move away from parsing nested CLI output.

Research notes:

- `Context` contains lazy clients, app context state, and caches; selected contexts should be treated as immutable launch inputs rather than mutable global targets.
- `Context::new` and environment/app resolution can have side effects such as manifest upgrades, app context initialization, component selection mutation, and server-side environment/app creation.
- Global or static state that matters for TUI concurrency includes CLI logging state, buffered logging, terminal width caching, program lookup caching, cargo target-dir caching, SDK override caching, and per-application agent type caches.
- Existing CLI logging uses global `LOG_STATE` and `LOG_STATE_BUFFER`; `LogIndent` and `LogOutput` mutate global state on construction/drop.
- Many command handlers hold logging scopes across `.await`, so a correct logging design must bind scopes to a context and preserve them across async execution.

Action And Help Rules completed.

Current status:

- Extended TUI action metadata with stable IDs, categories, scopes, execution kind, and palette visibility.
- Kept `TuiActionKind` as the local execution enum while using `TuiActionId` for UI derivation.
- Made palette search use palette-visible actions and include category, scope, and execution kind text.
- Derived footer shortcuts, normal leader hints, REPL leader hints, and help content from registered actions.
- Kept raw input controls such as scrolling, filtering, pane focus, and stdin routing as explicit context-help rows.
- Made `?` reachable from Agent inspect mode.
- Added a small test-only `TuiTestDriver` for key input and configurable frame assertions.
- Added focused tests for unique action IDs, palette visibility/category search, footer and leader drift, global help, Agent inspect help, REPL leader help, and command interaction help.

Validation:

- `cargo test -p golem-cli --lib -- tui::`
- `cargo check -p golem-cli`

Review follow-up:

- The action/help cleanup is enough for now, but a later DX/UX hardening goal should design a lightweight documentation system around TUI actions, shortcuts, help surfaces, and interaction conventions.
- That hardening work is intentionally not next; it should come after the fundamentals are stable, especially scoped logging/context execution and the first direct typed ops refresh path.

Scoped logging foundation implemented.

Current status:

- Replaced the single global CLI log state with a global fallback `LogContext` plus Tokio task-local scoped contexts.
- Added captured logging output for future direct TUI handler execution.
- Made `LogIndent` and `LogOutput` restore the context they were created against, including when dropped from another async scope.
- Added scoped `LogContext::scope`, `LogContext::spawn`, `buffered_lines`, and `take_buffered_lines`.
- Kept existing logging call sites working through the active context fallback.
- Left the context executor and direct Agents refresh for the next implementation slice.

Validation:

- `cargo fmt --package golem-cli`
- `cargo test -p golem-cli --lib -- log --report-time`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli --lib -- tui:: --report-time`

Context executor foundation implemented.

Current status:

- Added a small concrete TUI context executor module for `Arc<Context>` with an initial launch context, stable context id, active Tokio runtime handle reuse, scoped log capture, and generic completion event mapping.
- Kept the TUI runtime path async: `tui::run` is awaited from command dispatch, events are delivered through bounded Tokio MPSC, and spinner/auto-refresh ticks run as Tokio tasks.
- Added bounded-event backpressure behavior: important events await or blocking-send into the queue, while spinner/auto-refresh ticks are best-effort and dropped when the queue is full.
- Switched the temporary nested Agents refresh subprocess from `spawn_blocking` plus `std::process::Command` to `tokio::process::Command`.
- Isolated truly blocking terminal and PTY reads to Tokio blocking workers while feeding the same async TUI event loop; TUI-owned raw PTY threads were removed.
- Factored nested CLI target-to-event mapping helpers with focused tests.
- Wired production TUI startup to create one executor from the initial `Arc<Context>` without creating a nested runtime.
- Removed the executor spawn counter and kept tests focused on delivered completion events.
- Kept request construction and `TuiEvent` mapping outside the executor so it does not become a central provider.
- Kept Agents refresh on the existing nested JSON CLI path for this slice.
- Added executor tests with real CLI contexts for launch context delivery, captured logs, separate concurrent log buffers, error delivery, and active runtime reuse.

Validation:

- `cargo fmt --package golem-cli`
- `cargo test -p golem-cli --lib -- context_executor --report-time`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli --lib -- tui:: --report-time`

## 2026-06-30

TUI design system discovery started.

Decisions recorded:

- Pause further implementation slicing until the TUI itself has a stronger design system.
- Do not write the initial design system document immediately.
- First run an interview with the user to settle the important product and interaction questions.
- Create the initial document only after the current implementation, desired dev workflow, desired ops explorer, navigation model, and layout rules are clear enough.
- Review the document with the user and repeat the interview/revision loop until the design system is accepted.

Current status:

- Updated the TUI backlog so the current goal is interview-driven design system discovery.
- Marked the existing `ui-system.md` as a lightweight snapshot, not the authoritative final design system.
- Kept implementation goals such as selected context, action availability, direct streaming providers, and module splitting behind the design-system review loop.
- Rewrote `ui-system.md` as the initial authoritative design-system document.
- Added interaction and shortcut rules after a second interview pass.
- Standardized shortcut notation on lowercase forms such as `ctrl+x`, `ctrl+p`,
  `esc`, `enter`, and `tab`; uppercase letters must not imply `shift` unless
  `shift` is explicitly part of the shortcut.
- Recorded the target interaction grammar: few globals, modified number-based
  workspace jumps, `ctrl+x` as a visible transient leader menu, leader-based
  run actions, `tab` for panel focus, `esc` for step-out, `ctrl+q` for quit,
  explicit REPL raw-input focus, text input ownership of printable keys, and
  arrow-first list/table navigation.
- Reordered the TUI backlog around the accepted design direction: design review
  remains blocking for major redesign; the next meaningful product slice is the
  Home/Dev/Ops workspace shell full remap; low-risk interaction metadata cleanup
  may happen before the shell; selected context UX follows the first shell
  remap; direct streaming providers, module splitting, and local observability
  follow afterward.

Interaction metadata cleanup implemented.

Current status:

- Standardized user-facing TUI shortcut notation to lowercase forms such as
  `ctrl+x`, `ctrl+p`, `esc`, `enter`, `tab`, and explicit `shift+...` when a
  shifted key is actually required.
- Marked typed Agents refresh as a direct action instead of nested CLI metadata.
- Added action availability results with reasons for finite commands while a
  command is running, Agents refresh while already running, and Agents refresh
  when the context executor is unavailable.
- Kept unavailable palette actions visible, muted, and annotated with the
  reason; unavailable palette actions do not execute.
- Updated help, leader hints, footer/status strings, palette rendering, and
  focused tests for the new notation and availability rules.

Validation:

- `cargo fmt --package golem-cli`
- `cargo test -p golem-cli --lib -- tui::`
- `cargo check -p golem-cli`

Design-system review pass completed.

Current status:

- Reviewed `ui-system.md` section by section and accepted it as the planning
  baseline for the Home, Dev, and Ops workspace shell remap.
- Recorded the accepted sections directly in the design-system document:
  core model, global shell, workspaces, layout/navigation,
  interaction/shortcuts, jobs/output, context/environments, actions/help, Ops
  data rules, implementation audit, roadmap, and accessibility/stability.
- Reclassified the remaining open questions as non-blocking deferred decisions.
  They should be resolved by the milestone that owns them, not before the first
  workspace-shell remap can be planned.
- Marked the design discovery goal and Milestone 0 review gate complete in the
  TUI backlog.
- Left Rust code unchanged for this pass.

Validation:

- `git diff --check -- .tui-plan/ui-system.md .tui-plan/tasks.md .tui-plan/progress.md`

Workspace shell full remap implemented.

Current status:

- Replaced the old Dashboard, Agents, Output, Server, and REPL top-level TUI
  views with Home, Dev, and Ops workspaces.
- Remapped number navigation to `1` Home, `2` Dev, and `3` Ops; `[` and `]`
  cycle workspaces, while `tab` cycles Dev panel focus.
- Added the first Dev workbench layout with REPL as the primary panel and
  Output, Server, and Agents as secondary panels, with focused-panel fallback
  rendering on narrow terminals.
- Routed build/deploy/clean to Dev Output, server actions to Dev Server, and
  REPL actions to Dev REPL while preserving existing nested CLI behavior.
- Remapped Ops to the existing full Agents explorer, preserving typed refresh,
  mode cycle, auto-refresh, fuzzy filtering, details, stale-result handling,
  errors, and inspect split behavior.
- Updated action metadata, palette, help, tabs, and tests to use
  workspace/panel vocabulary instead of old top-level view names.
- Kept the initial TUI context model unchanged; selected-context switching is
  still the next milestone.

Validation:

- `cargo fmt --package golem-cli`
- `cargo test -p golem-cli --lib -- tui::`
- `cargo check -p golem-cli`

Global local server service foundation implemented.

Current status:

- Reclassified local server as a launch-scoped TUI service instead of a
  selected-context dev job.
- Kept server controls and logs visible through Dev while allowing the service
  to survive workspace and selected-context changes.
- Removed local server from context-switch blockers and dev-stop confirmation.
- Kept build/deploy/clean and REPL as context-bound jobs that still require
  confirmation before a selected-context switch can stop them.
- Made server start/restart use launch-scoped CLI args rather than the current
  selected Ops context args.
- Added a follow-up pointer-native layout milestone for mouse-position-aware
  scrolling, clickable focus/selection, split resizing, layout presets, and
  server side-panel decisions.

Validation:

- `cargo fmt --package golem-cli`
- `cargo check -p golem-cli`
- `cargo test -p golem-cli --lib -- tui:: --report-time`
