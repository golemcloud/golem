# TUI Product Contract

This document is the authoritative product contract for the rebuilt Golem TUI.
It records decisions that are locked for implementation and names the remaining
planning gates. Visual styling belongs in ui-system.md; implementation structure
belongs in architecture.md; executable work belongs in tasks.md.

## Status

The visual foundation, rebuilt Dev/Ops shell, complete local Dev loop, and
read-only Ops foundation are implemented. Ops includes the live Agents
collection, individual Agent Overview, current Deployment, Environment, and
Server overviews, direct bounded oplog snapshots, Deployment and Environment
resource catalogs, and a real built-in-local Metrics provider. Ops mutations,
Deployment History/Diff, deeper resource drill-downs, and transient Live remain
later work.

Selected-target identity, launch arguments, environment reference, and Server
identity now move together, and a context switch to a different Server target
requires stopping the supervised Server or cancelling the switch. Candidate
discovery still uses resolved CLI flags where the current CLI has no neutral
target materializer. Final manual terminal acceptance also remains outstanding.

The minimal local Metrics acquisition decision is locked: the built-in `golem`
server owns a bounded local collector and store. This does not select a general
remote observability architecture. A companion Docker stack or web UI is not
part of the minimal flow and will be considered only when it provides a clear
Golem-specific benefit.

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

- The header is context-only and exposes app/env/server. Workspace identity and
  activity do not compete with context in this row.
- The compact header uses the same app/env/server grammar and drops lower
  priority metadata before inventing another identity format.
- The footer stays compact and never duplicates a shortcut on multiple rows.
  Its workspace row publishes named Server and REPL lifecycle state in Dev and
  Ops inside one `─( … )─` cluster when width permits; the selector always has
  priority.
- Clicking a Server or REPL status shows and focuses its pane. Clicking the
  same visible, focused status again hides that pane without stopping its
  session; the final visible Dev pane cannot be hidden.
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

- ctrl+p opens Commands on the scope last used in the current TUI session. A
  fresh session starts on All.
- ctrl+x a opens the same Commands overlay on the Context scope.
- Left and right switch between All and Context without closing the overlay or
  clearing the query.
- Palette scope is also mouse-selectable.
- Context scope includes relevant unavailable actions and their reasons.
- All scope includes navigation, context, layout, Jobs, resource actions, and
  system commands.
- Modified shortcuts remain active while palette, quick-find, or form text is
  being edited. A nested terminal owns every key except ctrl+x only after the
  user explicitly gives it input ownership.

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
- Direct navigation shows and focuses its target without hiding another view or
  changing the primary view.
- Layout controls toggle any of the three views and can promote the focused
  view to primary without changing the visible set. At least one view remains
  visible.
- Hidden Server and REPL sessions continue running.

### Layout control

- Mouse dragging resizes every internal divider.
- A right-side split owns a standalone one-cell vertical rail with a short
  orange grip. A bottom split reuses the downstream pane's existing title row;
  the complete title rule is draggable and a click without movement focuses
  the pane.
- ctrl+x l enters temporary Layout mode.
- Tab and shift+tab choose a pane in Layout mode.
- Arrows move the applicable divider by one cell in the physical direction;
  shift+arrow moves five cells.
- Layout mode can flip right/bottom placement, show or hide Dev views or Ops
  details, and make the focused Dev view primary.
- Layout-mode hints expose only controls valid for the active workspace.
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
- Launching Build & Deploy, Server, or REPL switches to Dev and reveals and
  focuses its owning view without promoting it or hiding another visible view.
  Finite commands and REPL then give input to their visible focused PTY. Server
  remains TUI-owned after launch so an unexpected prompt cannot capture shell
  navigation. Every pane labels the owner as CLI or TUI. While CLI owns input,
  every key, including tab and ctrl+c, reaches the PTY except ctrl+x.
- ctrl+x returns input to persistent TUI control. Enter or a click in the
  running terminal pane returns input to the CLI. A second ctrl+x from TUI
  control opens the visible leader menu.
- Terminal summaries name the current input owner and launch identity without
  repeating handoff shortcuts already present in the footer help.
- Mouse wheel scrolls terminal history. Explicit Scrollback mode gives arrows,
  pageup, pagedown, home, and end to history; esc returns to live input.
- Terminal scrollback is bounded to 2,000 lines. Build & Deploy retains the
  latest 20 completed Jobs plus active Jobs for the current TUI session.
- A first Stop is graceful. Force Stop becomes available after five seconds
  and is never automatic.

### Build & Deploy

- Plan, Build, Deploy, and Clean are independent application-wide actions.
  Component selectors are deliberately omitted.
- Build reviews skip-check, force-build, and per-launch --yes. Internal
  half-public build-step selection is deliberately omitted.
- Plan and Deploy review full-diff, force-build, rollback version or revision,
  one post-deploy Agent policy, and per-launch --yes. Plan invokes deploy with
  --plan; it is not a prerequisite for Deploy.
- Clean removes all application build artifacts and reviews per-launch --yes.
- Form values are remembered per context for the TUI session after launch.
  Run is the first, centered, initially selected action; options remain in the
  standard selectable table below it. Cancel is transactional, and --yes
  always reopens disabled.
- Per-launch --yes is the first option for Build, Plan, Deploy, Clean, and
  REPL. Server places clean then --yes before its value fields.
- Enter starts editing a free-text value with a visible terminal cursor;
  arrows, home/end, backspace, delete, and ctrl+u edit the draft. Enter applies
  the draft and esc discards it without changing the reviewed value.
- Only one Plan, Build, Deploy, or Clean command may run for an application at
  a time. Later commands are unavailable rather than queued.
- This slice enables Dev commands only for manifest-backed local contexts.
  Remote Dev commands remain visible with a safety-planning reason.

### Server

- Lifecycle and view controls are separate. Commands expose Start while
  stopped or failed, Stop and Restart while starting or running, and Force Stop
  while stopping. Force Stop remains unavailable until the five-second grace
  period ends.
- Start and Restart always review system-memory override, router address,
  router/custom/MCP ports, ports file, data directory, clean,
  agent-filesystem root, and per-launch --yes.
- Ports file is optional and empty by default. When omitted, the TUI does not
  pass `--ports-file`; bound ports remain `not observed`. When supplied, a
  freshly rewritten parseable file enriches the displayed port details.
- A successful child-process spawn changes Starting to Running. Port-file
  observation never gates or substitutes for process lifecycle state.
- Restart first stops the current process, then launches the reviewed settings.
  Cancelling the form leaves the current server untouched.
- Server readiness is process-and-bound-port evidence only. No HTTP health
  probe or deeper readiness claim belongs in this slice.

### REPL

- REPL uses the same Start, Stop, Restart, and delayed Force Stop lifecycle
  vocabulary as Server. Showing or focusing the REPL is a separate navigation
  action.
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

The implemented Ops slice is intentionally read-only beyond the existing
Agents collection:

- Enter opens an individual Agent Overview and esc unwinds to Agents.
- Deployment Overview shows current deployment identity. Resources shows the
  current Components, Agent Types, HTTP/MCP deployments, deployed Tools, and
  Tool Middleware as typed homogeneous collections.
- Environment Overview shows selected-environment identity and configuration.
  Resources shows Secrets, Retry Policies, Resource Definitions, Domains,
  Security Schemes, and plugin/tool/middleware Grants without exposing secret
  values.
- Server Overview shows selected target provenance, endpoints, transport
  configuration, and matching TUI-owned local process/bound-port state.
- These providers call neutral typed handler/client operations directly. They
  preserve the CLI cache behavior and change shared caching only when a tested
  TUI freshness requirement exposes a mismatch.
- Read-only environment discovery must never use a get-or-create helper.
- Deployment History and revision Diff, standalone Component and Agent Type
  subjects, deeper resource item documents, and all Ops mutations remain
  discoverable only where a clear unavailable reason can be shown.

### View matrix

| Subject | Views |
|---|---|
| Agents collection | Overview, Activity, Metrics |
| Individual Agent | Overview, Activity, Metrics |
| Deployment | Overview, Resources, History |
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
resource definitions, domains, security schemes, and plugin/tool/middleware
grants. Deployment Resources owns Components, Agent Types, HTTP/MCP API
deployments, Tools, and Tool Middleware. Selecting a kind produces a
homogeneous collection and safe details projection. Account-level resources
are part of the future resource contract but not the minimal local UI.

### Collection and details contract

- Primary resource tables use one visual line per row.
- Long values use content-aware widths, ellipsis, horizontal pan, and full
  values in Details.
- Columns are stable, toggleable, and persisted as preferences.
- Numeric columns are right-aligned. AgentID receives the largest useful share
  of constrained widths before lower-priority identity columns.
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
- The active Agent mode filter is always visible beside the Dataset and Find
  state.

The accepted table visuals remain in force: odd/even row surfaces, distinct
selected odd/even colors, restrained colored selection fill, a continuous
accent selection rail, semantic status colors, AgentID syntax coloring,
unpadded cell separators where enabled, a consistent outer-left spine, and
structurally reserved right scrollbars.
Visible scrollbars accept thumb dragging and track clicks for their own pane;
their hit regions never resize an adjacent split.

## Activity

Activity has three representations:

- Timeline interprets and groups durable oplog events for diagnosis and is the
  default for every new context.
- Journal shows exact durable entries with AgentID, component, per-agent oplog
  index, timestamp, entry kind, and syntax-highlighted payload.
- Live shows transient stream output for one concrete Agent only.

Timeline and Journal are implemented from direct typed oplog snapshots. The
current bound is the latest 200 entries per Agent, with a soft warning above 10 Agents, a
hard maximum of 50 Agents, and at most four Agent reads in flight. Individual
failures produce a partial-result warning. Live remains visible as a locked
representation but truthfully reports that its direct transient stream provider
is not implemented; bounded oplog refresh is never labelled Live.

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
- Focused deterministic metric fixtures may be introduced only for an active
  UI review and must be unmistakably synthetic. None are retained currently;
  Production never mixes invented measurements with live context data.

### Minimal local provider

- Local Metrics is enabled by default with `localServer.observability: true`.
  `golem server run --observability true|false` is the explicit run override.
- Generated manifests use `localServer.dataDir: .golem/local-server-data`,
  ignore `/.golem`, and show how to opt out. Existing manifests without the
  field retain the enabled default.
- The built-in `golem` server samples its Prometheus registry every five
  seconds and accepts authenticated OTLP/HTTP JSON metrics on `/v1/metrics`.
- On a normal local deploy, the CLI discovers the private capability and adds
  the built-in metrics exporter only when that Agent Type has no explicit
  exporter configuration. Existing deployments are not rewritten at server
  startup and begin exporting on their next normal deploy.
- The store is `<dataDir>/observability.db`, with seven-day and 1 GiB bounds.
  `golem server clean --observability-only` removes it without deleting other
  local-server data.
- Private authenticated capability/family/series/sample routes back the TUI.
  The public `/metrics` endpoint and public HTTP API contract are unchanged.
- The first receiver accepts numeric OTLP JSON sum and gauge points with valid
  Golem Component and Agent identity. Known Golem families are categorized;
  other valid numeric families appear under Other. OTLP protobuf, Agent
  histogram/exponential-histogram/summary points, and custom SDK metrics are
  deferred rather than silently approximated.
- Metrics Now uses a fixed 15-minute range and refreshes every five seconds.
  Agents, individual Agent, and Server subjects are supported. Range,
  aggregation, follow, arbitrary filters, remote providers, Docker, and a web
  UI remain follow-up decisions.

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
- exact action coverage and risk class for each minimal resource subject;
- richer Activity scope resolution, cursor paging, Timeline interpretation,
  and transient Live lifecycle;
- generalized Metrics range, aggregation, auto-refresh, follow, and filter
  controls beyond the fixed local Now-15m slice;
- remote Metrics providers and companion tooling, only when a non-local Golem
  workflow justifies them.

When finite Jobs or TUI-owned sessions are active, ctrl+q opens a compact
choice with Return selected by default and Force quit as the explicit second
choice. The two choices share one horizontal row because left/right changes the
selection. Force quit stops every TUI-owned child process during normal cleanup.
