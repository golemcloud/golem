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
- [x] Defer Metrics acquisition and companion UI until the TUI contract passes
  visual and terminal acceptance.

### Planning gates still open

- [x] Lock the content hierarchy, controls, empty/running/error states, and
  contextual actions for Build & Deploy, Server, and REPL.
- [ ] Lock wide, short, narrow, and extreme header/footer priority rules,
  including Jobs, Live, and remote-lock status.
- [x] Lock active-work quit behavior: Return is safe/default; Force quit is
  explicit and stops all TUI-owned child processes during cleanup.
- [ ] Inventory exact contextual action coverage and risk class for each
  minimal resource subject.
- [ ] Choose Activity soft/default and hard maximum Agent counts plus overflow
  review behavior.
- [ ] Lock Metrics time-range, aggregation, refresh, auto-refresh, and follow
  controls without assuming a backend.
- [ ] Review the typed provider models and finalize implementation ordering at
  the end of the TUI-only design session.
- [-] Choose OTLP receiver, storage, retention, query backend, Docker stack, or
  web UI only after TUI acceptance.

The rebuild is not globally decision-complete until every non-deferred planning
gate is checked. The accepted local Dev and read-only Ops slice may proceed
while later mutation, Activity, Metrics, final shell priorities, and Live
integration remain open.

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
  preserve query while switching scope.
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
  stacked title rows also own their resize boundary instead of adding a second
  horizontal rule.
- [x] Implement temporary Layout mode, mouse dragging, one-cell and five-cell
  keyboard movement, placement flip, focus-only, show/hide, clamping, and
  compact retention.
- [x] Persist columns, placement, and ratios in the CLI user configuration area;
  do not write project files.
- [x] Render all shell/layout states at wide, short, narrow, compact, and 1x1
  sizes and test hit rectangles, dividers, spines, and scrollbars.

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
- [~] Apply remote write unlock and risk policy to Deploy modes; never retain a
  global yes/reset toggle. Per-launch review and local-only availability are
  live; remote unlock is deliberately still gated.

### Server

- [x] Show local availability, process state, resolved ports, data
  directory, launch mode, and bounded logs.
- [x] Support start, stop, restart, and clean-start through reviewed actions.
- [~] Preserve the same server across workspace/context changes only when the
  server target identity remains the same. Same-target preservation works;
  cross-target switching still needs pre-switch stop/cancel enforcement.

### REPL

- [x] Show stopped/starting/running/failed states and an explicit Start action.
- [x] Preserve PTY screen, resize, scrollback, and process state while hidden.
- [x] Make raw input focus explicit and retain a reliable leader escape back to
  TUI input.

### Dev acceptance

- [x] Test every primary view with zero, one, and two subviews in right/bottom
  and compact layouts.
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
- [ ] Add Deployment Overview/History and revision Overview/Diff.
- [ ] Add Environment Overview/Resources, resource-kind catalogs, homogeneous
  collections, masked secret handling, and contextual CRUD where planned.
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
- [x] Show current Deployment identity without implementing History or Diff.
- [x] Show selected Environment identity/configuration without Resources.
- [x] Show Server provenance/endpoints and matching local process/port state,
  without an HTTP readiness claim or lifecycle duplication.
- [x] Keep all Ops writes unavailable until their inventory and risk review.

## Checkpoint 6: Activity

Planning prerequisite: concrete Agent-scope bounds.

- [ ] Replace nested agent oplog execution with direct typed cursor/index
  providers.
- [ ] Implement explicit signal scope initialized from, but not bound to, the
  current Agents selection.
- [ ] Resolve included Agents and exact AgentID/Component/Agent Type filters to
  a reviewed bounded set.
- [ ] Implement Timeline mapping with retained AgentID and oplog index.
- [ ] Implement exact Journal rows, payload details, per-Agent cursors, bounded
  older/newer loading, and visible ordering semantics.
- [ ] Replace nested agent stream execution with one exact direct Live session.
- [ ] Keep Live connected visibly across navigation, provide close/clear/go-to
  actions, replacement confirmation, bounded scrollback, and bounded retry.
- [ ] Disable Live for multi-Agent scope without stopping an existing session.
- [ ] Test late/interleaved entries, duplicate timestamps, partial Agent
  failures, paging, context changes, disconnect/retry, Agent deletion, and
  compact rendering.

Recommended commit checkpoints: Journal provider; Timeline mapping; Live.

## Checkpoint 7: Provider-Independent Metrics UI

Planning prerequisite: Metrics control gate. Acquisition remains deferred.

- [x] Replace the production fake explorer with truthful source availability
  and retire the synthetic OTLP lab until Metrics UI review resumes.
- [ ] Define typed metric-family, concrete-series, sample, attributes, scope,
  time-coverage, and availability models.
- [ ] Implement family collection, series drill-down, sample/details document,
  sparklines, filters, range controls, and visible refresh state.
- [ ] Expose Golem service and Agent OTLP as separate sources with unsupported,
  unconfigured, unreachable, no-match, and no-samples states.
- [ ] Apply explicit Agent scope without enumerating unbounded agents when a
  provider can query equivalent attributes directly.
- [ ] Add Agents, Agent, Agent Type, and Server Metrics views from the shared
  primitives.
- [ ] Test family/series cardinality, long attributes, empty/error states,
  range coverage, auto-refresh, focus, details placement, and compact layouts.
- [ ] Complete a terminal visual review before choosing acquisition/storage.

Recommended commit checkpoint: real-provider-ready Metrics UI with truthful
unavailable production state.

## Checkpoint 8: Metrics Acquisition Decision

- [-] Validate real Agent exporter connectivity and capture OTLP payload/batch
  shapes after Checkpoint 7 acceptance.
- [-] Compare direct Prometheus scraping, an external standard stack, and an
  embedded receiver/store against the accepted TUI query contract.
- [-] Decide retention, pruning, size limits, query semantics, degradation, and
  whether Docker or a browser companion adds enough value.
- [-] Create a separate decision-complete implementation plan for GOL-162 data
  acquisition. Do not inherit the removed observability.db decision by default.

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
