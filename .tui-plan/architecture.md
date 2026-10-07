# Architecture

This document defines the internal boundaries required to implement the product
contract, including the selected minimal built-in-local Metrics backend.

## Runtime

The runtime owns terminal setup and one async event loop over:

- keyboard, mouse, paste, focus, and resize events;
- direct provider completions and stream messages;
- finite Job output, status, and process exits;
- Server, REPL, and Live session events;
- scheduled spinners, retries, and auto-refresh ticks.

Redraw after state-changing events instead of polling the renderer. Terminal
input and lifecycle/control completions have priority over a separately bounded
nested-process output channel, so sustained logs apply backpressure without
starving keyboard or mouse input. Replaceable animation ticks may be dropped.
Terminal input and provider work remain async. Portable PTY reads, waits,
writes, resize, and kill calls stay behind the existing isolated blocking
adapter.

Every async completion carries both a context identity and request generation.
Views discard stale results rather than applying data from a context or query
that is no longer active.

Nested CLI process traffic uses one typed event envelope containing the process
owner (finite command, Server, or REPL) and lifecycle event (output, output
closure, or exit). The runtime routes that envelope through one dispatch point;
do not add parallel top-level event variants for each process owner.

## Product State

The application state has these independent axes:

- selected context: app, environment, server, target provenance, and local or
  remote classification;
- workspace: Dev or Ops initially, represented as an ordered collection that
  can grow later;
- Dev layout: primary view, visible subviews, right or bottom placement,
  focused pane, and split ratios;
- Ops navigation: root or drill-down subject, subject-specific view,
  breadcrumbs, focused pane, and collection/detail state;
- overlays and modes: global or contextual palette, Jobs, help, context picker,
  confirmation, form input, and temporary Layout mode;
- finite Jobs and persistent Server, REPL, and Live sessions;
- persisted visual preferences and session-only per-context state.

Do not preserve the obsolete Home workspace, four named Dev layout presets,
global server drawer, or Ops-only product topology as compatibility paths.

## Context And Capability Model

The initial CLI Context is bootstrap input. The TUI owns a selected target that
can materialize the clients and application model required by a view or action.
The target descriptor retains:

- app and environment identity when selected;
- server endpoint, auth/source provenance, and local or remote classification;
- manifest/project identity when available;
- the resolved global flags needed by nested CLI or PTY work;
- a stable context identity and generation.

Each Dev view and Ops subject declares capability requirements and an
availability reason. Build & Deploy requires a manifest project, Server
requires a local target, and REPL requires an app environment plus reachable
server. Ops capabilities are subject-specific.

Every provider request, Job, and persistent session captures an immutable
launch context. Context switching never changes an existing launch target.
Switching either stops context-bound work after confirmation or preserves a
Server session only when the server target is identical.

The selected target owns its display identity, nested-CLI arguments,
environment reference, and stable Server identity as one value. Context-switch
requests capture that material immutably, so completion never re-reads a picker
selection that may have changed. Candidate discovery may still use resolved
`GolemCliGlobalFlags` until neutral target materializers exist, but those flags
must be converted into the selected-target value at the switch boundary. Do not
add fallback parsing or parallel selected-target contracts.

## Scoped Logging And Context Execution

Reuse the existing scoped LogContext and context executor for direct async
work. A launch helper installs the logging context and tracing span, executes a
caller-owned future against an immutable launch target, and emits a
caller-defined completion event.

The executor must not absorb view mapping or create its own Tokio runtime.
Views own request construction, typed result mapping, generation checks, and
state transitions.

CLI integrations expose neutral typed helpers. Do not add methods named for the
TUI and do not parse human or ANSI CLI output in refresh-heavy Ops views.
Interactive Dev workflows may continue to use nested CLI or PTY execution where
that preserves prompts and terminal behavior.

## Actions And Palettes

One action registry is the source of truth for:

- stable action ID and long label;
- global or contextual palette visibility;
- workspace, subject, view, and focus scope;
- hierarchical leader path;
- risk class;
- target resolution;
- availability and disabled reason;
- execution kind and input/review flow.

Global and contextual palettes are two filters over this registry. Palette
scope is remembered for the TUI session; ctrl+p reopens it, ctrl+x a explicitly
selects Context, and switching scope preserves the query. Search ranking is a
deterministic tiered comparison over visible labels and visible row metadata,
with registry order as the final tie-breaker. Help, leader hints, footer hints,
action tables, and availability tests derive from the same metadata where
practical.

Action execution never bypasses risk policy. A contextual action resolves its
immutable target before presenting review. Remote write unlock is a
session-only capability bound to one context identity; it is revoked on context
change and never stored as a preference.

## Jobs And Sessions

### Finite Jobs

A Job represents a finite command or mutation and records:

- ID, kind, risk class, and lifecycle state;
- immutable launch target and human-readable target summary;
- action inputs or command summary with secrets masked;
- structured progress or captured output;
- start/end times, exit state, and error;
- cancellation state and bounded session history.

The Jobs overlay reads one global registry. Build & Deploy renders a filtered
view of the same records. A per-environment mutation guard allows one active
mutating Job and makes later mutations unavailable; it does not queue them.
Read-only requests and signal subscriptions are not blocked by this guard.

### Persistent sessions

Server, REPL, and Live have independent lifecycle models because they are not
finite Jobs:

- Server owns process control, bound ports, data directory, and terminal
  buffer. Successful process spawn establishes Running; an optional fresh
  ports file may add bound-port details but never gates lifecycle or represents
  HTTP health.
- Each nested command or session owns a PTY screen, input mode, scrollback,
  resize, and process lifecycle through the same terminal interaction model.
- Live owns exact Agent identity, connection/retry state, follow state, and a
  bounded transient buffer.

Persistent sessions publish named compact status in the footer workspace row
and remain addressable from the palette. Clicking Server or REPL status shows
and focuses its Dev view without promoting it or handing input to the child; a
second click while that view is visible and focused routes through the shared
Dev-view toggle and hides it when another Dev view can remain.
Sessions are stopped according to context-switch and quit policies, not because
their pane becomes hidden.
Pointer workspace navigation first restores TUI keyboard ownership, so a stale
terminal-input mode cannot survive a Dev/Ops click.

Plan, Build, Deploy, Clean, Server, and interactive REPL use the nested CLI PTY
adapter. Other Ops operations use neutral typed helpers from existing handlers
or clients, retain their cache semantics, and never parse human or ANSI output.
If a TUI freshness rule conflicts with an existing cache, change the shared
cache contract with focused tests instead of bypassing it in the renderer.

Every Dev launch switches to Dev and reveals and focuses its owning view without
promoting it or hiding another visible view. Finite command and REPL launches
then give keyboard ownership to the focused PTY. Server launches remain
TUI-owned until Enter or a pointer click in the visible owning pane explicitly
hands input to the child. Each PTY has one TerminalScreen with
2,000 lines of scrollback and is resized to
the exact visible terminal rectangle from LayoutSnapshot after every layout
change, not only outer-terminal resize. A finite Job keeps its terminal
after completion. The registry keeps active Jobs and the latest 20 completed
Jobs, evicting the oldest completed record first.

## Layout

Rendering computes one LayoutSnapshot containing every visible pane, title,
content rectangle, scrollbar slot, boundary, split handle, overlay row, and hit
target. Mouse input routes only through snapshot region IDs.

`UiGeometry` owns the latest rendered snapshot. Rendering replaces it as one
unit, while pointer routing, PTY resize, and keyboard layout operations read
regions through its typed accessors. Application code must not keep a second
geometry cache or reach through the cache's interior mutability directly.
Each rendered scrollbar registers its exact pane and slot in this geometry.
Scrollbar track clicks and drags route before split handling and update that
pane's logical offset or selection; they cannot begin pane resize.

Dev layout is derived from primary view, visible set, and placement:

- one visible view fills the body;
- two visible views use one primary and one subview;
- three visible views use one primary and a two-pane subview region;
- right placement stacks subviews vertically;
- bottom placement arranges subviews horizontally.

Ops collection layouts use one collection pane plus an optional right or bottom
Details pane. Both layout families use the same recursive split representation,
ratio clamping, mouse dragging, compact fallback, and keyboard Layout mode.
Right splits reserve a disjoint one-cell vertical rail. Bottom splits reuse the
downstream pane title row as their divider, so that typed split region
intentionally overlaps the title and wins initial hit testing. Drag movement
resizes; release without movement routes through the title to focus the pane.
PaneChrome renders a short accent grip only on standalone vertical rails.

Layout mode selects a focused leaf and moves the unique applicable ancestor
divider in the requested physical direction. Unsupported directions leave
state unchanged and show a short reason. Ratios are expressed independently of
terminal dimensions, clamped to minimum cell sizes during layout, and retained
when compact rendering temporarily hides panes.

## Reusable UI Blocks

Product views compose shared blocks rather than drawing local variants:

- shell identity, workspace selector, breadcrumbs, global status, and footer;
- pane title, boundaries, structural scrollbar, and resize handle;
- cursor-backed collection query/status bar and one-line semantic table;
- focusable scrollable Details document and colored JSON/AgentID rendering;
- aligned notice, status, field, form, help, and action rows;
- global/context palette, chooser, confirmation, and blocking overlays;
- structured Job output, terminal screen, and signal sample/event details.

The existing widget and PaneRegion foundation remains authoritative. A pane
region is computed once and owns content width, scrollbar, divider boundaries,
and hit metadata.

Review forms share typed field identifiers, field-order tables, a centered Run
button, and `ReviewFieldRow`. Free-text fields use a transactional editor with
Unicode-safe byte boundaries, horizontal viewport tracking, visible cursor,
Enter-to-apply, and esc-to-discard. Styled popup notices are wrapped before
layout so their centered rows, form cursor, and content-sized height agree.

## Collections And Paging

Each collection provider returns typed rows plus typed continuation state. The
view owns:

- exact server Dataset scope;
- local loaded-row Find scope and query;
- stable selection and explicit inclusion identities;
- loaded depth and continuation state;
- refresh/loading/error/stale generations;
- table columns, horizontal pan, viewport, and Details state.

Continuation is always explicit and bounded. Refresh reloads the requested
depth without discovering another batch. Navigation and mouse scrolling never
trigger network requests. Bulk include/exclude affects loaded Find matches only
unless an action explicitly resolves and reviews a server-side scope.

## Ops Subject Providers

Use subject-specific typed providers behind a small shared lifecycle contract:

- load or refresh a collection/detail snapshot;
- continue a bounded collection where supported;
- return capability and availability information;
- start or stop a typed subscription where supported;
- resolve a contextual action target.

Do not create a universal data-provider enum that absorbs resource-specific
queries. Agents, Deployment, Environment Resources, Server, Activity, and
Metrics keep their own request and response types.

The initial read-only Ops providers are deliberately small. Deployment derives
the current revision/version/hash from the selected environment snapshot;
Environment returns its identity and configuration; Server derives immutable
endpoint/provenance data from the selected target and may join matching
TUI-owned local-session state. Initial environment lookup uses visible/get-only
APIs and must not invoke application or environment creation.

### Activity

Timeline and Journal use the worker oplog API directly. Each selected Agent has
independent cursor/index state. Journal preserves exact entries. Timeline maps
those entries into interpreted events and may merge by timestamp for display,
while retaining AgentID and oplog index.

A signal-scope resolver converts explicit Agent identities or a server-side
AgentID/Component/Agent Type filter into a reviewed bounded Agent set. The
resolver reports total or more-available truth supported by the API and never
silently chooses an unreviewed unbounded set.

Live must use the direct Agent connection rather than nested CLI output. It is
a single-Agent persistent session and is not inserted into the durable Journal.
That provider is not implemented yet; the current UI reports this explicitly
and does not substitute periodically refreshed oplog snapshots.

### Metrics

Metrics depends on a provider-neutral query contract that can:

- report source capability and availability;
- discover metric families under a subject and explicit scope;
- page or bound concrete series for a family;
- return series identity, attributes, type, units, samples, and observed time
  coverage;
- refresh a selected range without inventing unavailable history.

Golem service Prometheus and Agent OTLP are separate sources at this boundary.
For the built-in local server, the selected provider is:

- a `local-observability` feature on the `golem` binary, enabled by default;
- an in-process sampler over the server Prometheus registry every five seconds;
- an authenticated OTLP/HTTP JSON `/v1/metrics` receiver for Agent sum/gauge
  points carrying Golem Component and Agent identity;
- a dedicated SQLite `<dataDir>/observability.db`, bounded by seven days and
  1 GiB;
- authenticated private capability, family, series, and sample endpoints used
  by the CLI/TUI typed client;
- a deploy-time overlay that adds the built-in metrics exporter only for local
  deployments without an explicitly configured exporter.

The collector starts and degrades independently from the main server. A store
failure produces a failed capability and discarded-point count rather than
changing the public `/metrics` endpoint or public REST contract. Existing
deployments are never mutated at server startup. The overlay is applied on the
next normal local deployment and is never written back to the manifest.

The initial query UI uses a fixed 15-minute range and five-second refresh.
Remote providers, OTLP protobuf, Agent histogram-style types, custom SDK metric
semantics, generalized filters/ranges/aggregation/follow, Docker, and a web UI
remain outside this local slice. Production never uses invented measurements.

## Persistence

Persist one versionless current preference representation; backward
compatibility and migrations are not required by repository policy. Store it in
the CLI user configuration area, never the application project. Persist only
visual preferences such as columns, placement, and split ratios.

Session state holds context, navigation, filters, selection, Jobs, signal
scope, Live, auto-refresh/follow, remote unlock, and per-context caches. Exiting
the process discards it.

## Module Direction

Extract stable seams as the rebuild reaches them:

- keep the optional preview host in `tui::preview` and production fixture
  construction in a feature-gated `tui::app::preview_fixture` child module;
- add a lab module only while concrete synthetic stories exist, never as an
  empty or dormant parallel renderer;
- product navigation and context capability state;
- action metadata, risk policy, and palette scopes;
- finite Jobs and persistent sessions;
- recursive layout state and Layout mode;
- reusable collections and Details;
- one module per mature Dev view and Ops subject;
- Activity scope/oplog/Live providers;
- provider-neutral Metrics models.

Avoid a broad mechanical split before behavior is covered. app.rs may
orchestrate the transition, but finished milestones should not add more
workspace-specific rendering and event handling to that file.
