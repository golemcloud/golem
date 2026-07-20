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

The global local server service is exposed as a right-side drawer across Home,
Dev, and Ops. The drawer is closed by default, toggled by action/shortcut, and
resizable for the current TUI session. It is the v1 primary full server surface;
when open, Dev should avoid rendering a second large Server log panel.
