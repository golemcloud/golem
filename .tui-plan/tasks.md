# TUI Tasks And Readiness

This is the active, executable TUI backlog. Completed historical work remains
in progress.md; this file describes only the current rebuild and its gates.

## Status Markers

- [ ] planned or not yet decided
- [~] in progress
- [x] complete and synchronized with its source of truth
- [!] blocked by an external dependency or repeated failure
- [-] deliberately deferred

## Product Readiness Ledger

### Locked

- [x] Use app/env/server as the global context identity.
- [x] Start with Dev and Ops workspaces while allowing more later.
- [x] Keep Dev and Ops state and processes alive across workspace switches.
- [x] Use Build & Deploy, Server, and REPL as the three Dev views.
- [x] Start Dev with one primary view and no subviews.
- [x] Support optional right or bottom subviews without named layout presets.
- [x] Make every split mouse-resizable and keyboard-resizable in four physical
  directions through temporary Layout mode.
- [x] Use a hierarchical ctrl+x namespace for Workspaces, Views, Layout,
  Actions, Jobs, and Context.
- [x] Keep tab and shift+tab exclusively for pane focus.
- [x] Provide global and contextual scopes inside one switchable palette.
- [x] Organize Ops as subject then view, with collection/details panes.
- [x] Use Agents, Deployment, Environment, and Server as minimal Ops roots.
- [x] Keep Components and Agent Types as drill-down subjects.
- [x] Standardize primary resource collections on one-line rows.
- [x] Make Enter open a resource subject while Details remains a preview.
- [x] Use explicit bounded multi-Agent scope for Activity and Metrics.
- [x] Provide Activity Timeline, Journal, and single-Agent Live; default to
  Timeline.
- [x] Keep one exact Live session running visibly across navigation until it is
  closed or context changes.
- [x] Organize Metrics as family then concrete series.
- [x] Keep Golem service metrics and Agent OTLP telemetry visibly distinct.
- [x] Expose relevant resource mutations with risk classes and a per-context
  remote write lock.
- [x] Use Jobs as a global finite-operation overlay; do not make it a workspace.
- [x] Allow one mutating Job per environment and do not queue later mutations.
- [x] Persist visual preferences only; keep operational state session-local.
- [x] Use an embedded bounded provider for the minimal built-in-local Metrics
  flow; defer remote acquisition, Docker, and companion UI until justified.

### Planning gates still open

- [x] Lock the content hierarchy, controls, empty/running/error states, and
  contextual actions for Build & Deploy, Server, and REPL.
- [ ] Lock wide, short, narrow, and extreme header/footer priority rules,
  including Jobs, Live, and remote-lock status.
- [x] Lock active-work quit behavior: Return is safe/default; Force quit is
  explicit and stops all TUI-owned child processes during cleanup.
- [ ] Inventory exact contextual action coverage and risk class for each
  minimal resource subject.
- [x] Choose Activity soft/default and hard maximum Agent counts plus overflow
  review behavior: warn above 10, reject above 50, read 200 entries per Agent,
  and allow four concurrent reads.
- [~] Lock Metrics time-range, aggregation, refresh, auto-refresh, and follow
  controls. Now 15m with five-second refresh is locked for the local slice;
  general controls remain open.
- [x] Review the typed provider models and implementation ordering for the
  local resource, Activity snapshot, and Metrics slices.
- [x] Choose the minimal local OTLP receiver, bounded storage, retention, and
  query backend. Remote providers, Docker, and web UI remain deferred.

The rebuild is not globally decision-complete until every non-deferred planning
gate is checked. The local Dev and read-only Ops foundation may proceed while
later mutations, generalized Metrics controls, final shell priorities, deeper
resource navigation, and Live integration remain open.

## Accepted Implementation Slice

- [x] Replace the legacy product topology with Dev and Ops plus the shared
  navigation/action/layout foundation.
- [x] Implement typed review forms and nested-CLI PTYs for Plan, Build, Deploy,
  Clean, Server, and interactive REPL.
- [x] Add bounded Jobs with 20 completed records, persistent Server/REPL
  sessions, pane-sized resize, and 2,000-line terminal scrollback.
- [x] Enable Dev execution only for manifest-backed local contexts; keep remote
  Dev mutations unavailable with reasons.
- [x] Migrate Agents into the subject shell and add individual Agent Overview.
- [x] Add real read-only Deployment, Environment, and Server overviews through
  typed operations that never create missing resources.
- [x] Move fake Metrics to preview/test data and render truthful production
  unavailability.
- [x] Offer safe Return and explicit Force quit while active work exists.
- [x] Keep Return and Force quit side by side so their layout matches
  left/right selection.
- [x] Prioritize terminal and control events over bounded nested-process output
  so sustained logs cannot starve interaction.
- [x] Add pane-owned scrollbar click/drag behavior without stealing split
  resize, and restore TUI ownership on pointer workspace changes.
- [x] Show Agent mode, protect AgentID width, and right-align numeric table
  columns.
- [x] Make Layout help workspace-specific and keep nested-terminal summaries
  free of duplicated handoff shortcuts.

## Checkpoint 0: Synchronize The Planning Sources

- [x] Add product-contract.md as the authoritative behavior contract.
- [x] Rewrite architecture.md around the accepted product and session model.
- [x] Replace the stale Ops-first phases and backlog with this readiness plan.
- [x] Reconcile ui-system.md, testing.md, dx-ux-review.md, and progress.md.
- [x] Run markdown/reference searches, stale-contract searches, and
  git diff --check.
- [x] Review the resulting readiness ledger with the user.

Recommended commit checkpoint: product contract and rebuild plan only.

## Checkpoint 1: Product State And Action Foundations

Planning prerequisites for the accepted slice are satisfied by the nested-CLI
Dev boundary and read-only Ops provider boundary. Mutation inventory remains a
gate for later Ops actions.

- [x] Replace the one-entry workspace constant with ordered Dev/Ops workspace
  state that can accept future workspaces.
- [x] Add explicit Dev view, Ops subject/view, breadcrumb, primary/subview,
  placement, focused-pane, and palette-scope state.
- [x] Introduce a selected target descriptor with stable identity, provenance,
  local/remote classification, per-view capability reasons, nested-CLI launch
  arguments, and environment reference. Candidate discovery may retain
  resolved CLI flags until neutral target materializers exist.
- [x] Extend action metadata with hierarchical leader path, palette relevance,
  risk class, target resolution, review requirement, and availability reason.
- [x] Make global and contextual palettes filters over the same action registry;
  preserve query while switching scope, remember scope for the session, and
  keep ctrl+x a as an explicit Context entry.
- [x] Rank typed palette queries by exact/prefix/word-prefix/substring/fuzzy
  tiers with deterministic availability/registry tie-breaking, and highlight
  matched characters in visible row text.
- [x] Keep modified shortcuts active inside text-editing modes; nested PTY input
  owns every key except the ctrl+x handoff to TUI control.
- [x] Add reducer/state tests for workspace, view, subject, breadcrumb, palette,
  capability, and stale-context transitions.
- [x] Remove replaced flat shortcut and workspace compatibility paths in the
  same change.

Recommended commit checkpoint: state/action model with behavior-preserving Ops
rendering.

## Checkpoint 2: Shared Shell And Recursive Layout

Planning prerequisite for full acceptance: shell priority gate. The accepted
slice implements normal/compact geometry and a safe, explicitly provisional
extreme fallback.

- [x] Use one responsive context-only header in Dev and Ops: full
  `app value · env value · server value`, compact `app/environment/server`, and
  no workspace, view, Job, Server-session, or REPL-session badges. Workspace
  selection stays in the footer; view and activity state stay in their owning
  pane or overlay.
- [x] Keep one compact footer; transient leader/Layout hints replace rather than
  append duplicate rows.
- [x] Implement ctrl+x w/v/l/a/j/e groups with visible nested hints, workspace
  next/previous/list, and direct Dev/Ops destinations.
- [x] Replace named Dev presets with recursive primary/subview layout state and
  right/bottom placement.
- [x] Reuse the recursive split model for Ops collection/details layouts.
- [x] Render Dev and Ops through the accepted shared pane titles, neutral
  spines/dividers, semantic status markers, and trailing-right scrollbars;
  stacked layouts reuse the downstream pane title row as the horizontal resize
  divider without adding a second rule.
- [x] Implement temporary Layout mode, mouse dragging, one-cell and five-cell
  keyboard movement, placement flip, focus-only, show/hide, clamping, and
  compact retention.
- [x] Persist columns, placement, and ratios in the CLI user configuration area;
  do not write project files.
- [x] Render all shell/layout states at wide, short, narrow, compact, and 1x1
  sizes and test hit rectangles, dividers, spines, and scrollbars.
- [x] Make the complete divider the pointer target, keep vertical rails
  disjoint from pane and scrollbar rectangles, render their short accent grip,
  and let a click without movement on a stacked title/divider focus that pane.

Recommended commit checkpoint: shell, navigation, layout, and preferences.

## Checkpoint 3: Jobs, Persistent Sessions, And Safety

Planning prerequisite for the full checkpoint: exact Ops action-risk inventory.
The accepted slice implements finite local Dev Jobs and explicit active-work
Return/Force quit behavior.

- [~] Introduce the global bounded Jobs registry for finite Dev commands and Ops
  mutations. Finite Dev commands are live; later Ops mutations must join the
  same registry.
- [~] Model structured result/output, cancellation, terminal states, duration,
  immutable target, and masked inputs. The Dev command terminal/lifecycle is
  complete; typed Ops results and generalized masked inputs remain later work.
- [x] Add the Jobs overlay, owning-pane status, global/context palette entries,
  and Build & Deploy filtered projection; global activity does not leak into
  the context-only header.
- [~] Enforce one active mutating Job per environment by disabling later
  mutations with the active Job named. The single active Dev command is
  enforced; the generalized per-environment guard awaits Ops mutations.
- [~] Separate Server, REPL, and Live persistent-session state from Jobs.
  Server and REPL are separate; Live belongs to the later Activity slice.
- [x] Add read-only, routine-write, disruptive, and destructive risk policies
  to the implemented action registry.
- [ ] Add session-only remote unlock by exact app/env, revoke it on context
  change/exit, and keep destructive confirmations cancel-focused.
- [ ] Resolve and display bulk target counts, consequences, and available plans
  or diffs before execution.
- [~] Implement context-switch and quit handling for every Job/session state.
  Dev Job, REPL, and cross-Server handling plus explicit force quit are live;
  the later Live session still needs integration with the same policy.
- [~] Test local/remote availability, unlock expiry, typed confirmation,
  concurrent-mutation rejection, cancellation, masking, and stale completions.
  Accepted-slice availability, forms, cancellation, and stale results are
  covered; remote unlock and Ops mutation cases remain gated.

Recommended commit checkpoint: Jobs and mutation safety before broad actions are
made reachable.

## Checkpoint 4: Dev Workspace Rebuild

Planning prerequisite: Dev view-content gate. Complete.

### Build & Deploy

- [x] Make Build & Deploy the first available Dev view and preserve the no-
  subview first-run default.
- [x] Expose Plan, Build, Deploy, and Clean through contextual actions and
  visible controls without bare global run keys.
- [x] Render the current or selected Job, exact nested-CLI terminal, status,
  error, exit metadata, and bounded history projection without parsing the
  human diff into a second representation.
- [x] Use typed review-field order, centered active Run, confirmation options
  first, and transactional cursor-backed free-text editing with Unicode-safe
  movement and Enter/apply versus esc/discard behavior.
- [~] Apply remote write unlock and risk policy to Deploy modes; never retain a
  global yes/reset toggle. Per-launch review and local-only availability are
  live; remote unlock is deliberately still gated.

### Server

- [x] Show local availability, process state, resolved ports, data
  directory, launch mode, and bounded logs.
- [x] Support state-specific start, stop, restart, and delayed force-stop
  actions; keep clean as an option in the reviewed start/restart form.
- [x] Start Server with TUI keyboard ownership, expose Enter-to-CLI input, omit
  ports-file by default, mark successful spawn Running independently of port
  observation, and treat an explicit fresh ports file as optional metadata.
- [x] Preserve the same server across workspace/context changes only when the
  server target identity remains the same, and enforce stop-or-cancel before a
  cross-target switch.

### REPL

- [x] Show stopped/starting/running/failed states and an explicit Start action.
- [x] Preserve PTY screen, resize, scrollback, and process state while hidden.
- [x] Make raw input focus explicit and retain a reliable leader escape back to
  TUI input.

### Dev acceptance

- [x] Test every primary view with zero, one, and two subviews in right/bottom
  and compact layouts.
- [x] Make direct Dev navigation show and focus its target without replacing
  the current primary or hiding visible panes; expose independent stable view
  toggles and focused-view promotion while keeping at least one view visible.
- [x] Reveal and focus the owning Dev view when Build & Deploy, Server, or REPL
  launches without promoting or hiding another pane. Finite commands and REPL
  receive CLI input; Server remains TUI-owned.
- [x] Align Server and REPL lifecycle actions with state-specific Start, Stop,
  Restart, and delayed Force Stop commands, and keep Server clean as a reviewed
  launch option rather than a separate lifecycle verb.
- [x] Show clickable Server and REPL state in the footer in both workspaces,
  with full state at normal widths and compact glyphs at narrower widths.
- [~] Test direct navigation to hidden and visible views, focus restoration,
  hidden-process continuity, context capability reasons, and resize behavior.
  Deterministic state/layout coverage is live; final terminal smoke testing is
  still required.
- [x] Remove the obsolete Output panel, Agents Dev panel, named presets, and
  global server drawer once replacements are live.

Recommended commit checkpoint: complete minimal Dev loop.

## Checkpoint 5: Ops Navigation And Resource Roots

Planning prerequisite for mutations: exact resource-action inventory. The
read-only root subset below may proceed first.

- [x] Add root-subject navigation for Agents, Deployment, Environment, and
  Server with subject-specific views and breadcrumbs.
- [x] Migrate live Agents Overview into the shared subject shell without
  regressing Dataset/Find, explicit inclusion, paging, columns, details,
  selection, scrolling, or refresh behavior.
- [x] Change Enter from Details toggle to subject drill-down and move Details
  visibility into Layout/actions.
- [~] Add Agent, Component, Agent Type, and Deployment Revision drill-down
  subjects with direct palette navigation. Individual Agent is live; the other
  drill-down subjects remain later work.
- [~] Add Deployment Overview/Resources/History and revision Overview/Diff.
  Overview plus current Components, Agent Types, HTTP/MCP deployments, Tools,
  and Tool Middleware catalogs are live; History/Diff remain open.
- [~] Add Environment Overview/Resources, resource-kind catalogs, homogeneous
  collections, masked secret handling, and contextual CRUD where planned.
  The read-only safe catalog is live; item drill-down and CRUD remain open.
- [x] Add Server Overview/capability data without duplicating Dev server
  lifecycle controls.
- [ ] Implement relevant Invoke, Interrupt, Resume, Update, Redeploy, Delete,
  and resource CRUD actions through typed helpers and Jobs.
- [~] Test breadcrumb unwind, palette direct jumps, unavailable providers,
  stale refresh retention, compact details, and local/remote action policy.
  The accepted read-only subset is covered; later subjects and writes are not.

Recommended commit checkpoints: Agents migration; Deployment; Environment;
Server and action coverage.

### Accepted Read-Only Subset

- [x] Preserve the complete Agents collection interaction model.
- [x] Open a read-only individual Agent Overview from Enter and remove the old
  nested oplog/stream Inspect path.
- [x] Show current Deployment identity and current typed resource catalog
  without implementing History or Diff.
- [x] Show selected Environment identity/configuration and a safe typed
  resource catalog without exposing secret values.
- [x] Show Server provenance/endpoints and matching local process/port state,
  without an HTTP readiness claim or lifecycle duplication.
- [x] Keep all Ops writes unavailable until their inventory and risk review.

## Checkpoint 6: Activity

Planning prerequisite: concrete Agent-scope bounds.

- [~] Replace nested agent oplog execution with direct typed cursor/index
  providers. Direct bounded snapshots are live; cursor paging is open.
- [~] Implement explicit signal scope initialized from, but not bound to, the
  current Agents selection. Selected and explicitly included Agent identities
  are snapshotted; a dedicated scope editor is open.
- [~] Resolve included Agents and exact AgentID/Component/Agent Type filters to
  a reviewed bounded set. Included identities use soft 10/hard 50 bounds and
  concurrency 4; server-side filter resolution remains open.
- [~] Implement Timeline mapping with retained AgentID and oplog index. Stable
  merged rows are live; richer interpretation/grouping is open.
- [~] Implement exact Journal rows, payload details, per-Agent cursors, bounded
  older/newer loading, and visible ordering semantics. Exact serialized details
  and identity/index are live; cursors and older/newer paging are open.
- [ ] Replace nested agent stream execution with one exact direct Live session.
- [ ] Keep Live connected visibly across navigation, provide close/clear/go-to
  actions, replacement confirmation, bounded scrollback, and bounded retry.
- [ ] Disable Live for multi-Agent scope without stopping an existing session.
- [ ] Test late/interleaved entries, duplicate timestamps, partial Agent
  failures, paging, context changes, disconnect/retry, Agent deletion, and
  compact rendering.

Recommended commit checkpoints: Journal provider; Timeline mapping; Live.

## Checkpoint 7: Provider-Independent Metrics UI

The fixed local first slice uses Now 15m and five-second refresh. General range,
aggregation, follow, and filter controls remain a review gate.

- [x] Replace the production fake explorer with real source availability and
  keep synthetic measurements out of production.
- [x] Define typed metric-family, concrete-series, sample, attributes, scope,
  time-coverage, and availability models.
- [~] Implement family collection, series drill-down, sample/details document,
  sparklines, filters, range controls, and visible refresh state. The fixed
  Now-15m path is live; arbitrary controls remain open.
- [~] Expose Golem service and Agent OTLP as separate sources with unsupported,
  unconfigured, unreachable, no-match, and no-samples states. The two local
  sources and core states are live; remote/provider-specific states remain open.
- [~] Apply explicit Agent scope without enumerating unbounded agents when a
  provider can query equivalent attributes directly. Individual Agent and
  local environment scopes are query filters; the scope editor remains open.
- [~] Add Agents, Agent, Agent Type, and Server Metrics views from the shared
  primitives. Agents, individual Agent, and Server are live; Agent Type awaits
  its subject.
- [ ] Test family/series cardinality, long attributes, empty/error states,
  range coverage, auto-refresh, focus, details placement, and compact layouts.
- [ ] Complete a terminal visual review before choosing acquisition/storage.

Recommended commit checkpoint: real-provider-ready Metrics UI with truthful
unavailable production state.

## Checkpoint 8: Minimal Local Metrics Acquisition

- [x] Add `localServer.observability`, the `golem server run --observability`
  override, template opt-out guidance, and `.golem/local-server-data`.
- [x] Add the default-on `local-observability` server feature, isolated startup
  failure, five-second service sampling, authenticated OTLP/HTTP JSON receiver,
  and private typed query routes without changing public `/metrics`.
- [x] Store points in `<dataDir>/observability.db` with seven-day and 1 GiB
  bounds, plus `golem server clean --observability-only`.
- [x] Add the normal-deploy local exporter overlay; explicit user exporter
  configuration wins and existing deployments change only on their next deploy.
- [~] Validate real Agent exporter connectivity and payload batches in the demo
  application. Parser and overlay tests exist; an installed-binary smoke test
  remains.
- [ ] Decide a remote provider only when a non-local workflow requires it.
  Docker and a browser companion remain unselected for the minimal local flow.

## Checkpoint 9: Cleanup And Final Acceptance

- [x] Gate the preview host and production fixture behind `tui-preview`;
  remove the synthetic catalog, fake OTLP explorer, visual candidates, and
  obsolete lab-only widget contracts.
- [x] Separate the real production preview fixture into
  `tui::app::preview_fixture`; do not create an empty lab module.
- [~] Remove dormant renderers, actions, state, fixtures, and compatibility code
  replaced by the rebuilt workspaces. Replaced production paths are gone; a
  final post-review audit remains.
- [ ] Split stable actions, context, Jobs/sessions, layout, Dev views, Ops
  subjects, Activity, and Metrics modules out of app.rs.
- [~] Keep every user-facing shortcut/help/footer entry derived or drift-tested
  against the action registry.
- [~] Run focused TUI tests after each checkpoint and the complete TUI library
  suite at final acceptance.
- [~] Run cargo check for golem-cli with tui-preview/tests as appropriate,
  non-mutating formatting checks, every registered production-preview size, and
  git diff --check.
- [ ] Install the matching dev-release binary and manually smoke test Dev/Ops,
  context switching, PTY cleanup, mouse routing, compact layout, and remote
  safeguards.

Recommended commit checkpoint: cleanup and accepted minimal Golem TUI.
