# Architecture

The TUI is a keyboard-first shell over existing Golem behavior. It should stay simple internally while moving duplicated rules into explicit models.

## Runtime

The runtime owns terminal setup and drives one async event loop over:

- terminal key, mouse, and resize events;
- background refresh completions;
- nested CLI/PTTY output and process exits;
- explicit scheduled events such as spinners or auto-refresh ticks.

The runtime should redraw only after events. Avoid a global polling render loop. The event channel should be bounded with an explicit capacity; high-value events should apply backpressure, while low-value ticks can be dropped when the queue is full. Terminal input should feed the event channel from async streams where possible, and blocking PTY reads should be isolated to blocking workers.

Terminal input should use async stream APIs where the terminal backend provides them. `portable-pty` does not expose Tokio-native process, reader, writer, resize, or kill APIs, so nested interactive PTY jobs may keep a small blocking adapter. That adapter must be isolated from view/event logic: long-running PTY reads and waits run on blocking workers, and TUI event handlers queue write/resize/kill requests through bounded async channels instead of calling PTY methods directly.

## Action Model

Actions are the source of truth for user-visible commands. Each action should have:

- stable ID;
- long label;
- optional shortcut;
- category or workspace: navigation, dev, ops, settings, REPL, system;
- scope: global, leader, view-specific, focused workflow, or system;
- availability rule and reason, when that becomes necessary;
- execution kind.

Palette rows, footer hints, leader hints, and help should derive from this model wherever possible. Raw input controls that are not actions yet, such as text entry or scroll keys, can remain explicit context-help controls.

## Context Model

The initial CLI `Context` should be treated as bootstrap input. The TUI needs an explicit selected context that can later represent:

- manifest environment;
- explicit local mode;
- explicit cloud mode;
- custom named environment;
- config-only or non-manifest mode.

Every nested job should capture an immutable launch context. Switching the selected UI context must not change the meaning of an already-running build, deploy, server, REPL, or inspection job.

Direct TUI queries should also capture an immutable launch context. The selected context can change later, but in-flight results must carry the launch context identity and request generation that produced them so stale results are ignored.

`Context` is not a pure value today. It contains lazy clients, app context state, and caches, and context construction can have manifest/config side effects. Future context switching should create or cache separate `Arc<Context>` values rather than mutating a live context in place.

## Logging Context

The CLI logging system is currently effectively global. That conflicts with concurrent TUI jobs that call command handlers directly, because one refresh should not steal another refresh's log output, indentation, or buffered error text.

Introduce a proper scoped logging context before broad direct handler use:

- a `LogContext` owns output mode, indentation, stashed indentation, and captured/buffered lines;
- `LogIndent` and `LogOutput` bind to the active context when they are created and restore that same context when dropped;
- log lookup checks a scoped context first and falls back to the existing global CLI behavior for normal command execution;
- async direct TUI work runs inside a logging scope and returns captured logs with the typed result when needed.

The scope should not be thread-local only. It needs async-aware propagation across `.await`, and spawned work should use an explicit helper that carries both the logging context and tracing span. A reasonable lookup order is tracing span extension, Tokio task-local, thread-local, then global fallback.

## Context Executor

Use a small TUI context executor rather than a central provider that absorbs every view's event logic.

The executor should:

- hold the selected `Arc<Context>` and context generation;
- clone a `TuiLaunchContext` for each spawned request;
- run a caller-provided async closure against that launch context on background runtime infrastructure;
- install the scoped logging context for that closure;
- send the caller-provided completion event back to the TUI loop;
- reuse the active Tokio runtime handle; it should not create or block on its own runtime.

Views stay responsible for request-specific decisions. For example, the Agents view should choose the agent list mode, call a neutral worker-list data helper, map typed rows into `AgentListItem`, and decide which `TuiEvent` to emit.

## Jobs

Nested CLI jobs should carry:

- job ID and kind;
- command line and working directory;
- launch context;
- PTY runtime;
- lifecycle state;
- output buffer or terminal screen;
- scroll/follow state;
- exit code and last error.

Finite commands, server, REPL, and agent inspect jobs should converge on shared lifecycle rules where practical, without forcing unrelated UI details into one abstraction.

## Dev And Ops Split

Dev workflows are application/manifest-oriented: build, deploy, clean, server, REPL, manifest exploration.

Ops workflows are environment/resource-oriented: agents, components, resources, logs, streams, inspections, and future observability.

The shell stays shared: context bar, tabs, palette, jobs/output, help, and notifications.

Dev workflows may keep using nested CLI/PTTY execution when that preserves interactive behavior, especially build, deploy, clean, server, and REPL flows. Ops and resource exploration workflows should use direct handler/client calls by default and must not parse nested CLI output for refresh-heavy views.

Nested CLI in ops views is only a temporary implementation gap. Each remaining use should be tracked explicitly until it has a typed provider or streaming provider replacement. Agent inspect oplog/stream remains transitional ops debt until a direct streaming/oplog provider is added.

Direct ops calls should run through the context executor, return typed results into the TUI event loop, carry an immutable target/context identity, and let views ignore stale results by generation.

`spawn_blocking` is not a general TUI orchestration tool. It is allowed only at explicitly named blocking-library adapter boundaries, currently the `portable-pty` adapter. Direct ops/provider work and non-PTY async orchestration should stay on Tokio async APIs.

Command handler integration should use neutral data-returning helpers. Do not add `for_tui` methods to handlers. Extract shared internal logic when needed so the existing CLI command path renders/logs results while TUI callers receive typed data directly.

## Module Direction

Do not start with a broad rewrite. Extract stable seams as they become useful:

- `actions` for action metadata and availability;
- `context` for selected/launch context;
- `jobs` for nested lifecycle state;
- `context_executor` or similarly named module for scoped context/logging/background execution;
- `layout` for named region computation, hit testing, split geometry, session-only layout ratios, and drawer geometry;
- `help` for derived context help;
- view modules after behavior is rule-driven enough to move safely.

## Layout And Pointer Routing

Rendering computes a `LayoutSnapshot` from the terminal area, active workspace,
mode, agents state, and session layout state. The snapshot owns named regions
for workspace tabs, workspace body, Dev panels, Ops list/details/inspect panes,
modal rows/actions, split handles, and the global local-server drawer.

Mouse handling must route through these region IDs. Wheel events scroll the
region under the pointer. Clicks focus or select the clicked region. Dragging a
split handle updates only in-memory ratios on `TuiApp`; no resize state is
persisted to config.

The first Dev layout presets are:

- right: REPL primary left, side panels right;
- left: side panels left, REPL primary right;
- top: side panels top, REPL primary bottom;
- bottom: REPL primary top, side panels bottom.

The earlier global local-server drawer is not part of the Ops-first production
surface. Its process and output primitives may be reused when a real Dev
workflow returns, but the drawer and its actions must not remain reachable only
for compatibility.

## Ops-First Product Model

The initial production shell contains one workspace, Ops, and starts there.
The footer renders `[Ops]` so the accepted workspace-selector geometry remains
without reserving an `alt`+number binding. Future workspaces must choose
keyboard-layout-safe navigation when they are introduced.

Ops separates the thing being inspected from the way it is inspected:

- subject: Agents initially; components and other Golem entity families can be
  added later without changing the view vocabulary;
- view: Overview and Metrics in production, followed by Activity when its data
  provider is ready;
- scope: the selected server plus optional app/environment from the global
  context header;
- selection: a focused table row is distinct from the explicit set of agents
  included in cross-agent analysis.

Agent Overview uses the existing typed list request. Refreshes retain explicit
selection identities rather than interpreting an incomplete cursor page as
deletion. Failed refreshes retain the previous rows and mark them stale.
Filtering may hide selected identities without silently clearing them; a later
cross-agent view must resolve and report unavailable selections explicitly.

Agent collection navigation has two deliberately separate layers:

- the dataset filter is server-side and exact. It selects all deployed agent
  components, one component, or one deployed agent type before rows are loaded.
  Its picker has case-insensitive literal type-ahead over component and agent
  type names, but applies the selected exact scope only on confirmation;
- Find is a local case-insensitive literal filter over one explicit field and
  only the rows currently loaded. Its active field and loaded-match count
  remain visible while it is active;
- the first request and every explicit continuation request are bounded to 200
  rows total across the selected dataset, not 200 per component. The typed page
  cursor retains both an unfinished component cursor and components that have
  not been scanned yet. The UI reports the loaded count and `more available`;
  it does not invent page numbers or a global total the API does not provide;
- continuation is explicit through `ctrl+l` or Commands. Reaching the end by
  keyboard, mouse selection, or mouse wheel never starts a request;
- include-all and exclude-all operate on loaded rows matching the local Find,
  never on unloaded server results. Explicit selections hidden by a Find remain
  selected until the user changes them.

Refreshes preserve the currently loaded depth so auto-refresh does not collapse
the collection back to its first batch. Changing the server dataset or agent
mode resets cursors and loaded depth before fetching the new dataset.

Reusable collection surfaces own query/status rows, cursor-backed table state,
semantic table cells, pane-header status, scrollable documents, colored JSON,
and resizable list/details splits. Product views compose these blocks and own
their request-specific mapping rather than recreating their rendering rules.
Cursor-backed state owns typed per-source progress, including not-yet-scanned
sources, the configured total batch size, and loaded depth. The view maps typed
results; it does not maintain a second, view-local notion of page depth.

Metrics may aggregate the complete matched set through a backend query. Oplog
and live activity require an explicit, bounded set of concrete agents. Limits
must be visible and user-adjustable only up to source-specific hard caps; the
UI must never silently pick the first or busiest agents.

Activity is one view with two representations. Timeline groups and interprets
diagnostic events. Journal exposes exact oplog entries, indexes, and payload
details. A merged multi-agent presentation retains component, agent, and oplog
index. Timestamp ordering is presentation only; per-agent oplog index remains
authoritative unless trace or invocation links establish causality.

## Local Metrics Direction

Until the live receiver and query contracts are implemented, Metrics contains a
deterministic fake OTLP explorer for layout and interaction review. Every demo
pane must say `FAKE`, the details must state that no receiver or query store was
read, and fake samples must never be mixed with or presented as live context
data.

The minimal local path is owned by the single `golem` process:

- keep the existing combined Prometheus `/metrics` surface for Golem service
  metrics;
- add a metrics-only OTLP HTTP receiver for agent exports;
- persist normalized samples in a dedicated bounded `observability.db` under
  the local server data directory, not in `registry.db`;
- accept metrics only in the first receiver; native logs and oplogs remain on
  their existing APIs, and OTLP traces/logs are not duplicated;
- resolve the current granted built-in OTLP exporter instead of hard-coding a
  plugin version;
- add the exporter only as an effective deployment overlay during a normal
  deploy/update, never by editing `golem.yaml` and never by revising components
  or agents at server startup;
- preserve a user-configured external exporter rather than overriding or
  attempting to install the same grant twice.

Existing agents on older revisions remain valid and show agent metrics as
unavailable until a normal update/deploy. Fixed local router ports use a
loopback connect host even when bound on all interfaces. Dynamic router ports
need an explicit degraded rule before the overlay can be enabled safely.

Before implementing ingestion, validate agent-to-router loopback and real OTLP
payload/batch shapes, then lock series identity, retention/pruning and database
size bounds, the local query contract, and receiver/exporter degradation
reporting. Supporting every distributed server topology is not a requirement
for this local-first slice.
