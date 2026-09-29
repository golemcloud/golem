# TUI Product Contract

This document is the authoritative product contract for the rebuilt Golem TUI.
It records decisions that are locked for implementation and names the remaining
planning gates. Visual styling belongs in ui-system.md; implementation structure
belongs in architecture.md; executable work belongs in tasks.md.

## Status

The visual foundation and the live Agents explorer are implemented. The next
implementation slice rebuilds the shared shell and the complete local Dev loop,
then adds read-only Deployment, Environment, Server, and individual Agent
overviews to Ops. Activity, real Metrics providers, resource catalogs, and Ops
mutations remain later work.

The metrics acquisition backend and any companion Docker or web UI are
deliberately undecided. They will be planned only after the provider-independent
TUI interactions have passed visual and terminal review.

## Vocabulary

- Context: the selected application, environment, and server, rendered as
  app/env/server in compact form.
- Workspace: a top-level product area. Dev and Ops are the initial workspaces;
  navigation must support more later.
- Dev view: Build & Deploy, Server, or REPL.
- Primary view: the Dev view occupying the main layout region.
- Subview: an optional secondary Dev view shown beside the primary view.
- Ops subject: the resource or collection being inspected.
- Ops view: the representation of a subject, such as Overview, Activity, or
  Metrics.
- Pane: a focusable rendered layout region. Ops details are panes, not Dev
  subviews.
- Job: a finite TUI-managed command or resource operation with status and
  bounded session history. Server, REPL, and Live are persistent sessions, not
  Jobs.

## Product Boundary

- Dev owns the application development loop: plan, build, deploy, clean, local
  server, and REPL.
- Ops owns deployed-resource inspection, activity, metrics, and contextual
  resource actions.
- Switching workspaces never changes context or stops processes.
- Shared resources are not duplicated into Dev and Ops variants. Dev may link
  to an Ops subject, but Ops owns the full explorer.
- Home, Server, REPL, Output, Jobs, Observe, Context, and Settings are not
  top-level workspaces.

## Global Context

- The selected target is always explained as app/env/server.
- Each request, Job, and persistent session captures an immutable launch target.
- Results carry context identity and generation so stale results can be ignored.
- Dev capabilities are evaluated per view:
  - Build & Deploy requires a manifest-backed application project.
- Server requires a local server target.
  - REPL requires a selected application environment and reachable server.
- Ops is available whenever the selected target contains enough information for
  the requested subject or action.
- Unavailable views and actions remain discoverable and show the reason.

### Context changes

- The minimal TUI has one active Dev context.
- Switching context confirms and stops active context-bound Jobs and REPL.
- A supervised local server continues only when the new context uses the same
  server target. Changing server target requires stopping it or cancelling the
  switch.
- Live closes on context change. Its retained buffer may remain in session
  state, but it does not reconnect automatically.
- Completed Jobs, filters, navigation, and layout may be cached per context for
  the current TUI session.

## Shell And Navigation

- The header exposes workspace, current view or subject, app/env/server, remote
  write-lock state, active Live state, and running or failed Jobs as space
  permits.
- The compact header uses the same app/env/server grammar and drops lower
  priority metadata before inventing another identity format.
- The footer stays compact and never duplicates a shortcut on multiple rows.
- Tab and shift+tab move pane focus only.
- Enter opens a selected resource subject. Esc closes a transient mode first,
  then unwinds one breadcrumb level.
- Bare printable keys belong to the focused input. Global commands require a
  modifier or the ctrl+x leader.
- ctrl+arrow and alt+number are prohibited. Bare space is allowed only in an
  explicitly focused chooser such as Columns.

The accepted normal shell uses one header row, three footer rows at normal
sizes, and two footer rows when width is below 72 cells or height is below 20
cells. The exact metadata priority at extreme widths and the final body/footer
priority at extreme heights remain a visual-review gate. Until that review, the
renderer must clip safely, retain a body row whenever geometry allows, and
avoid inventing another context identity format.

### Hierarchical leader

ctrl+x opens a visible hierarchical leader with these stable groups:

- w: workspaces, including next, previous, list, and direct mnemonic targets;
- v: views within the current workspace or subject;
- l: temporary Layout mode;
- a: contextual actions;
- j: Jobs;
- e: context.

The workspace group must scale beyond Dev and Ops. Individual destinations live
inside the group rather than consuming the top-level ctrl+x namespace.

### Palettes

- ctrl+p opens the global palette.
- ctrl+x a opens the contextual action palette.
- While a palette is open, those same shortcuts switch between All and Context
  scopes without closing the overlay or clearing the query.
- Palette scope is also mouse-selectable.
- Context scope includes relevant unavailable actions and their reasons.
- All scope includes navigation, context, layout, Jobs, resource actions, and
  system commands.
- Modified shortcuts remain active while palette, quick-find, or form text is
  being edited. REPL raw input is the explicit exception.

## Dev Workspace

### Views

- Build & Deploy owns Plan, Build, Deploy, Clean, their structured results, and
  bounded session history.
- Server owns local-server lifecycle, process state, resolved bound ports, data
  directory, and logs. It does not claim HTTP readiness.
- REPL owns the persistent interactive application REPL.

The first Dev visit uses Build & Deploy when available, otherwise the first
available view. It opens with no subviews. Returning from Ops restores the exact
Dev view, focus, visible subviews, placement, and ratios from the current
session.

### Dev layout

- One view is always primary.
- Any non-primary Dev view may be shown as a subview.
- Subviews are placed either to the right or at the bottom. There are no named
  layout presets.
- With three visible views, right-side subviews stack vertically and bottom
  subviews arrange horizontally.
- Direct navigation focuses a visible target. A hidden target replaces the
  primary without increasing the visible pane count.
- Hidden Server and REPL sessions continue running.
- Focus Only hides subviews without stopping them.

### Layout control

- Mouse dragging resizes every internal divider.
- ctrl+x l enters temporary Layout mode.
- Tab and shift+tab choose a pane in Layout mode.
- Arrows move the applicable divider by one cell in the physical direction;
  shift+arrow moves five cells.
- Layout mode can flip right/bottom placement and show or hide subviews or Ops
  details.
- Enter or esc exits Layout mode.
- Minimum sizes prevent unusable panes.
- Compact mode renders one pane while retaining wider-layout preferences.

### Dev command execution

- Plan, Build, Deploy, Clean, Server, and interactive REPL run the real nested
  CLI in a PTY. The TUI provides typed review forms and process/session state;
  it does not reimplement those command handlers.
- Each launch uses an immutable context, the application root as working
  directory, and a fresh terminal. The PTY receives the actual pane content
  size rather than the outer terminal size.
- The terminal owns printable input, arrows, Enter, and ctrl+c while focused.
  ctrl+x escapes to TUI commands; ctrl+x ctrl+x forwards a literal ctrl+x.
- Mouse wheel scrolls terminal history. Explicit Scrollback mode gives arrows,
  pageup, pagedown, home, and end to history; esc returns to live input.
- Terminal scrollback is bounded to 2,000 lines. Build & Deploy retains the
  latest 20 completed Jobs plus active Jobs for the current TUI session.
- A first Stop is graceful. Force Stop becomes available after five seconds
  and is never automatic.

### Build & Deploy

- Plan, Build, Deploy, and Clean are independent application-wide actions.
  Component selectors are deliberately omitted.
- Build reviews build steps, skip-check, force-build, and per-launch --yes.
- Plan and Deploy review full-diff, force-build, rollback version or revision,
  one post-deploy Agent policy, and per-launch --yes. Plan invokes deploy with
  --plan; it is not a prerequisite for Deploy.
- Clean removes all application build artifacts and reviews per-launch --yes.
- Form values are remembered per context for the TUI session after launch.
  Cancel is transactional, and --yes always reopens disabled.
- Only one Plan, Build, Deploy, or Clean command may run for an application at
  a time. Later commands are unavailable rather than queued.
- This slice enables Dev commands only for manifest-backed local contexts.
  Remote Dev commands remain visible with a safety-planning reason.

### Server

- Start and Restart always review system-memory override, router address,
  router/custom/MCP ports, ports file, data directory, clean,
  agent-filesystem root, and per-launch --yes.
- When no ports file is supplied, the TUI supplies one from the CLI user
  configuration area. Starting changes to running only after the child process
  remains alive and rewrites a parseable ports file after launch.
- Restart first stops the current process, then launches the reviewed settings.
  Cancelling the form leaves the current server untouched.
- Server readiness is process-and-bound-port evidence only. No HTTP health
  probe or deeper readiness claim belongs in this slice.

### REPL

- Start and Restart always review language, log streaming, automatic imports,
  one post-deploy Agent policy, and per-launch --yes.
- The embedded view runs interactive REPL only. Script and script-file modes
  remain standalone CLI commands, and component/revision selectors are omitted.
- The REPL continues while hidden and retains its terminal and scrollback until
  stopped or replaced by a fresh launch.

## Ops Workspace

Ops navigation is subject then view then pane.

### Root subjects

- Agents
- Deployment
- Environment
- Server

Components, Agent Types, individual Agents, Deployment Revisions, and
environment resource kinds or items are drill-down subjects. Components and
Agent Types remain reachable through breadcrumbs and the palette without
becoming permanent root entries.

The first rebuilt Ops slice is intentionally read-only beyond the existing
Agents collection:

- Enter opens an individual Agent Overview and esc unwinds to Agents.
- Deployment Overview shows the current deployment identity only.
- Environment Overview shows selected-environment identity and configuration.
- Server Overview shows selected target provenance, endpoints, transport
  configuration, and matching TUI-owned local process/bound-port state.
- These providers call neutral typed handler/client operations directly. They
  preserve the CLI cache behavior and change shared caching only when a tested
  TUI freshness requirement exposes a mismatch.
- Read-only environment discovery must never use a get-or-create helper.
- Deployment History, revision Diff, Environment Resources, Components, Agent
  Types, Activity, and all Ops mutations remain discoverable only where a clear
  unavailable reason can be shown.

### View matrix

| Subject | Views |
|---|---|
| Agents collection | Overview, Activity, Metrics |
| Individual Agent | Overview, Activity, Metrics |
| Deployment | Overview, History |
| Deployment Revision | Overview, Diff |
| Environment | Overview, Resources |
| Server | Overview, Metrics |
| Component | Overview, Agent Types, Agents |
| Agent Type | Overview, Agents, Activity, Metrics |
| Environment resource kind or item | Collection or Overview |

Deployment History is a revision collection. Opening a revision exposes its
Overview and Diff; Diff defaults to the previous revision and may compare to an
explicitly selected revision.

Environment Resources is a catalog of resource kinds: secrets, retry policies,
quota or resource definitions, API deployments, domains, security schemes,
tools, middleware, and grants. Opening a kind produces a homogeneous collection.
Account-level resources are part of the future resource contract but not the
minimal local UI.

### Collection and details contract

- Primary resource tables use one visual line per row.
- Long values use content-aware widths, ellipsis, horizontal pan, and full
  values in Details.
- Columns are stable, toggleable, and persisted as preferences.
- Details follow selection as a preview. Enter opens the selected subject.
- Details may be placed right or bottom, focused, independently scrolled, and
  resized.
- Compact mode shows the collection and details one at a time.
- Server Dataset filters and local loaded-row Find are separate and visibly
  labelled.
- Requests and continuations are bounded. Loading always means one more batch,
  never all remaining resources.
- Refresh preserves stable selection identity, loaded depth, and viewport when
  possible. Failed refresh retains stale rows and explains the error.

The accepted table visuals remain in force: odd/even row surfaces, distinct
selected odd/even colors, restrained colored selection fill, a continuous
accent selection rail, semantic status colors, AgentID syntax coloring,
unpadded cell separators where enabled, a consistent outer-left spine, and
structurally reserved right scrollbars.

## Activity

Activity has three representations:

- Timeline interprets and groups durable oplog events for diagnosis and is the
  default for every new context.
- Journal shows exact durable entries with AgentID, component, per-agent oplog
  index, timestamp, entry kind, and syntax-highlighted payload.
- Live shows transient stream output for one concrete Agent only.

Timeline and Journal share one explicit signal scope. Timestamp merging is a
presentation order only; per-agent oplog index is authoritative unless a real
trace or invocation link establishes causality.

### Signal scope

- The default is the Agent selected when Activity is opened. It becomes an
  explicit snapshot; later table movement does not retarget it.
- Users may include concrete Agents or define an exact server-side AgentID,
  Component, or Agent Type filter.
- Activity resolves a filter to a visible bounded set before fan-out.
- A match above the safe bound is reported and must be narrowed or explicitly
  reduced to a bounded batch.
- Local Find never silently changes Activity or Metrics scope.

### Live session

- Live is bound to one immutable app/env/server/AgentID and never follows table
  selection or Timeline/Journal scope.
- One explicitly started Live session may continue across views and workspaces.
- Its global status names the Agent and connection state.
- The user can close it from the header, Activity, contextual palette, or
  global palette. Closing retains bounded scrollback until clear or exit.
- Starting Live for another Agent requires replacing the existing session.
- Multi-agent scope disables the Live representation but does not silently stop
  an existing background Live session.
- Connection loss uses visible bounded retry/backoff. Agent deletion shows a
  terminal close reason.

## Metrics

- Metrics first lists metric families, not every concrete label set.
- Opening a family lists its concrete series. Opening a series shows complete
  attributes, Golem identity, samples, current value, and a sparkline.
- Filters cover metric name, source, type, AgentID, Component, Agent Type, and
  attributes where the provider supports them.
- Golem Prometheus service metrics and Agent OTLP telemetry remain visibly
  distinct sources.
- Availability distinguishes unsupported, unconfigured, unreachable,
  configured with no matching data, and no samples in the selected range.
- Deterministic fake metrics are preview and test fixtures only. Production
  never mixes them with live context data.

Metrics acquisition, retention, query storage, Docker integration, and a
possible web UI remain a post-TUI decision. The UI must depend on a typed
provider boundary rather than a chosen storage implementation.

## Actions And Safety

Ops will expose relevant CLI parity for implemented subjects, including Invoke,
Interrupt, Resume, Update, Redeploy, Delete, and resource-specific create,
update, or delete operations. Parity applies to resources present in the TUI;
it does not require every CLI command before its subject exists.

Every action declares one risk class:

- read-only;
- routine write;
- disruptive;
- destructive.

Read-only actions execute directly. Routine writes review exact target and
inputs. Disruptive actions require explicit confirmation. Destructive actions
use a cancel-focused confirmation and typed target.

### Remote write lock

- Non-local contexts start with changes locked.
- Enabling changes requires typing the exact app/env.
- Unlock applies only to that context and TUI session.
- Context switch or exit revokes it.
- Per-action confirmations still apply after unlock.
- Bulk and destructive actions show the exact target, resolved count,
  consequences, and available plan or diff.
- Local contexts skip the session unlock but retain disruptive and destructive
  confirmations.
- The TUI never stores a global yes flag or remote-write preference.
- Local versus remote is resolved from context metadata, never hostname
  heuristics.

## Jobs And Persistent Sessions

- Jobs is a global overlay, not a workspace.
- It contains finite Dev commands and Ops resource operations.
- Build & Deploy embeds its relevant Job subset.
- The header exposes running and failed Jobs.
- Selecting a Job shows target, status, structured result, output, duration,
  exit state, and error.
- Job history is bounded and session-only.
- The initial bound is the latest 20 completed Jobs plus every active Job.
- Only one mutating Job may run for an application environment. Other
  mutations are temporarily unavailable; they are not queued automatically.
- Read-only refreshes and signal subscriptions remain independent.
- Server, REPL, and Live are persistent sessions with their own lifecycle and
  global status, not indefinite Jobs.

## Persistence

Persist only visual preferences across TUI restarts:

- visible optional columns;
- right or bottom pane placement;
- divider ratios;
- equivalent visual layout choices added later.

Keep context, selected workspace or subject, filters, breadcrumbs, table
selection, Jobs, signal scopes, time ranges, follow or auto-refresh, Live,
remote unlock, and Timeline/Journal selection session-only. Within a session,
state may be cached per context. UI preference storage must not edit the
application manifest or project tree.

## Remaining Planning Gates

The product direction is locked, but implementation readiness still depends on
these decision groups, which tasks.md tracks before their checkpoints:

- final wide, short, narrow, and extreme header/footer priority rules,
  including Jobs, Live, and remote-lock status;
- quit behavior for active Jobs and TUI-owned persistent sessions;
- exact action coverage and risk class for each minimal resource subject;
- concrete soft/default and hard bounds plus overflow review for multi-agent
  Activity;
- Metrics time-range, aggregation, refresh, auto-refresh, and follow controls;
- typed provider interfaces and final implementation ordering;
- Metrics acquisition and companion tooling, deliberately postponed until the
  provider-independent TUI passes review.

Until quit behavior is planned, quitting is unavailable while a finite Job or
TUI-owned Server or REPL session is active and explains that the work must be
stopped first. This is a conservative implementation constraint, not the final
quit policy.
