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

- [ ] Lock the content hierarchy, controls, empty/running/error states, and
  contextual actions for Build & Deploy, Server, and REPL.
- [ ] Lock wide, short, narrow, and extreme header/footer priority rules,
  including Jobs, Live, and remote-lock status.
- [ ] Lock quit behavior for active Jobs and TUI-owned Server, REPL, and Live
  sessions.
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

The rebuild is not decision-complete until every non-deferred planning gate is
checked. Implementation may proceed milestone by milestone only when the gate
owned by that milestone is closed.

## Checkpoint 0: Synchronize The Planning Sources

- [x] Add product-contract.md as the authoritative behavior contract.
- [x] Rewrite architecture.md around the accepted product and session model.
- [x] Replace the stale Ops-first phases and backlog with this readiness plan.
- [x] Reconcile ui-system.md, testing.md, dx-ux-review.md, and progress.md.
- [x] Run markdown/reference searches, stale-contract searches, and
  git diff --check.
- [~] Review the resulting readiness ledger with the user.

Recommended commit checkpoint: product contract and rebuild plan only.

## Checkpoint 1: Product State And Action Foundations

Planning prerequisites: provider/order gate and exact resource-action inventory.

- [ ] Replace the one-entry workspace constant with ordered Dev/Ops workspace
  state that can accept future workspaces.
- [ ] Add explicit Dev view, Ops subject/view, breadcrumb, primary/subview,
  placement, focused-pane, and palette-scope state.
- [ ] Introduce a selected target descriptor with stable identity, provenance,
  local/remote classification, and per-view capability reasons.
- [ ] Extend action metadata with hierarchical leader path, palette relevance,
  risk class, target resolution, review requirement, and availability reason.
- [ ] Make global and contextual palettes filters over the same action registry;
  preserve query while switching scope.
- [ ] Keep modified shortcuts active inside every text-editing mode except REPL
  PTY input.
- [ ] Add reducer/state tests for workspace, view, subject, breadcrumb, palette,
  capability, and stale-context transitions.
- [ ] Remove replaced flat shortcut and workspace compatibility paths in the
  same change.

Recommended commit checkpoint: state/action model with behavior-preserving Ops
rendering.

## Checkpoint 2: Shared Shell And Recursive Layout

Planning prerequisite: shell priority gate.

- [ ] Render Dev/Ops selectors, current view or subject, app/env/server, Jobs,
  Live, and remote-lock status through one responsive header model.
- [ ] Keep one compact footer; transient leader/Layout hints replace rather than
  append duplicate rows.
- [ ] Implement ctrl+x w/v/l/a/j/e groups with visible nested hints, workspace
  next/previous/list, and direct Dev/Ops destinations.
- [ ] Replace named Dev presets with recursive primary/subview layout state and
  right/bottom placement.
- [ ] Reuse the recursive split model for Ops collection/details layouts.
- [ ] Implement temporary Layout mode, mouse dragging, one-cell and five-cell
  keyboard movement, placement flip, focus-only, show/hide, clamping, and
  compact retention.
- [ ] Persist columns, placement, and ratios in the CLI user configuration area;
  do not write project files.
- [ ] Render all shell/layout states at wide, short, narrow, compact, and 1x1
  sizes and test hit rectangles, dividers, spines, and scrollbars.

Recommended commit checkpoint: shell, navigation, layout, and preferences.

## Checkpoint 3: Jobs, Persistent Sessions, And Safety

Planning prerequisites: quit policy and exact action-risk inventory.

- [ ] Introduce the global bounded Jobs registry for finite Dev commands and Ops
  mutations.
- [ ] Model structured result/output, cancellation, terminal states, duration,
  immutable target, and masked inputs.
- [ ] Add the Jobs overlay, header status, global/context palette entries, and
  Build & Deploy filtered projection.
- [ ] Enforce one active mutating Job per environment by disabling later
  mutations with the active Job named.
- [ ] Separate Server, REPL, and Live persistent-session state from Jobs.
- [ ] Add read-only, routine-write, disruptive, and destructive risk policies.
- [ ] Add session-only remote unlock by exact app/env, revoke it on context
  change/exit, and keep destructive confirmations cancel-focused.
- [ ] Resolve and display bulk target counts, consequences, and available plans
  or diffs before execution.
- [ ] Implement context-switch and quit handling for every Job/session state.
- [ ] Test local/remote availability, unlock expiry, typed confirmation,
  concurrent-mutation rejection, cancellation, masking, and stale completions.

Recommended commit checkpoint: Jobs and mutation safety before broad actions are
made reachable.

## Checkpoint 4: Dev Workspace Rebuild

Planning prerequisite: Dev view-content gate.

### Build & Deploy

- [ ] Make Build & Deploy the first available Dev view and preserve the no-
  subview first-run default.
- [ ] Expose Plan, Build, Deploy, and Clean through contextual actions and
  visible controls without bare global run keys.
- [ ] Render the current or selected Job, structured Plan/diff result, status,
  output, error, and bounded history projection.
- [ ] Apply remote write unlock and risk policy to Deploy modes; never retain a
  global yes/reset toggle.

### Server

- [ ] Show local availability, process state, health, resolved ports, data
  directory, launch mode, and bounded logs.
- [ ] Support start, stop, restart, and clean-start through reviewed actions.
- [ ] Preserve the same server across workspace/context changes only when the
  server target identity remains the same.

### REPL

- [ ] Show stopped/starting/running/failed states and an explicit Start action.
- [ ] Preserve PTY screen, resize, scrollback, and process state while hidden.
- [ ] Make raw input focus explicit and retain a reliable leader escape back to
  TUI input.

### Dev acceptance

- [ ] Test every primary view with zero, one, and two subviews in right/bottom
  and compact layouts.
- [ ] Test direct navigation to hidden and visible views, focus restoration,
  hidden-process continuity, context capability reasons, and resize behavior.
- [ ] Remove the obsolete Output panel, Agents Dev panel, named presets, and
  global server drawer once replacements are live.

Recommended commit checkpoint: complete minimal Dev loop.

## Checkpoint 5: Ops Navigation And Resource Roots

Planning prerequisite: exact resource-action inventory.

- [ ] Add root-subject navigation for Agents, Deployment, Environment, and
  Server with subject-specific views and breadcrumbs.
- [ ] Migrate live Agents Overview into the shared subject shell without
  regressing Dataset/Find, explicit inclusion, paging, columns, details,
  selection, scrolling, or refresh behavior.
- [ ] Change Enter from Details toggle to subject drill-down and move Details
  visibility into Layout/actions.
- [ ] Add Agent, Component, Agent Type, and Deployment Revision drill-down
  subjects with direct palette navigation.
- [ ] Add Deployment Overview/History and revision Overview/Diff.
- [ ] Add Environment Overview/Resources, resource-kind catalogs, homogeneous
  collections, masked secret handling, and contextual CRUD where planned.
- [ ] Add Server Overview/health/capability data without duplicating Dev server
  lifecycle controls.
- [ ] Implement relevant Invoke, Interrupt, Resume, Update, Redeploy, Delete,
  and resource CRUD actions through typed helpers and Jobs.
- [ ] Test breadcrumb unwind, palette direct jumps, unavailable providers,
  stale refresh retention, compact details, and local/remote action policy.

Recommended commit checkpoints: Agents migration; Deployment; Environment;
Server and action coverage.

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

- [ ] Replace the production fake explorer with truthful source availability;
  retain deterministic samples only in preview/test providers.
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

- [ ] Remove dormant renderers, actions, state, fixtures, and compatibility code
  replaced by the rebuilt workspaces.
- [ ] Split stable actions, context, Jobs/sessions, layout, Dev views, Ops
  subjects, Activity, and Metrics modules out of app.rs.
- [ ] Keep every user-facing shortcut/help/footer entry derived or drift-tested
  against the action registry.
- [ ] Run focused TUI tests after each checkpoint and the complete TUI library
  suite at final acceptance.
- [ ] Run cargo check for golem-cli with tui-preview/tests as appropriate,
  non-mutating formatting checks, every deterministic story size, and
  git diff --check.
- [ ] Install the matching dev-release binary and manually smoke test Dev/Ops,
  context switching, PTY cleanup, mouse routing, compact layout, and remote
  safeguards.

Recommended commit checkpoint: cleanup and accepted minimal Golem TUI.
