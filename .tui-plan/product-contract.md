# TUI Product Contract

## Product Boundary

- Dev owns Plan, Build, Deploy, Clean, local Server, and REPL. These execute the
  real nested CLI in PTYs.
- Ops owns deployed-state inspection, durable history, live Agent events,
  metrics, deployment inspection, and configuration inventory.
- The first rebuilt Ops milestone is read-only. It does not delete, interrupt,
  update, roll back, deploy, or mutate configuration.
- Safe non-interactive operations reuse neutral command-handler helpers or
  typed clients. Ops never shells out to another CLI.
- Switching workspaces does not change context or stop a process.

## Global Shell

- The header is one context-only row using `app/env/server`.
- The footer is shared by Dev and Ops and continues to expose Server and REPL
  lifecycle state even when their panes are hidden.
- Bare printable keys belong to the focused control. Global actions require a
  modifier or the visible `ctrl+x` leader.
- `tab` and `shift+tab` move pane focus. `enter` drills into a selected entity;
  `esc` closes a transient mode or unwinds one route level.
- `ctrl+arrow` and `alt+number` are not shortcuts.

## Ops Navigation

The permanent destinations are:

1. Overview
2. Agents
3. Oplog
4. Stream
5. Metrics
6. Deployments
7. Configuration

- They render as one horizontally scrollable, mouse-selectable row. Narrow
  terminals retain full labels and show overflow markers.
- `ctrl+x v o/a/l/s/m/d/c` opens the corresponding destination and
  `ctrl+x v n/p` moves to the next or previous destination.
- The first Ops visit opens Overview. Later workspace switches restore the
  active destination and its session state.
- Each destination owns an in-tab route stack. Breadcrumbs expose depth;
  returning restores collection selection and scroll.
- Merely selecting an Agent never silently retargets another destination.
  `Open selected Agent in …` is an explicit scope handoff.
- Context changes keep the active destination but return it to its root, clear
  operational scopes and live buffers, cancel stale requests, and reload.

## Shared Collection Contract

- `Filter` changes the server-side dataset and resets its cursor.
- `Find loaded` performs highlighted literal matching over fetched rows only.
- If no loaded match exists while a backend cursor remains, the UI offers an
  explicit server search; it never escalates silently.
- Status always distinguishes loaded, shown, more available, and end reached.
- Paging loads one bounded batch. No action named “load all” is allowed.
- Refresh retains stale content and distinguishes initial loading, refreshing,
  empty, unavailable, partial, stale, and failed states.
- Details placement and ratio persist per destination. Query, selection,
  cursor, and live state do not persist across TUI sessions.

## Overview

Overview is a lightweight status-and-routing cockpit. Its status block uses
only the current environment/deployment identity, TUI-owned local Server state,
the local observability capability, and retained Stream state. Opening Overview
must not query a Metrics dashboard, enumerate Agents, or fetch full deployment
revision composition.

A stable quick-jump directory opens Agents, Oplog, Stream, Metrics,
Deployments, Configuration, nested Server/collector details, and actionable
Issues. Metrics opens the Agent dashboard at environment scope and 15 minutes;
other destinations retain their own explicit scope contracts. Oplog and Stream
may therefore open unscoped and explain that the user must choose an Agent in
Agents. The Overview never presents loaded Agent rows as a total Agent count.

## Agents

- Agents remains a paged, single-row semantic table with toggleable columns,
  optional right/bottom Details, AgentID-prioritized width, status colors,
  keyboard/mouse selection, and selection sets.
- Find defaults to AgentID. Component and Agent Type are explicit fields; there
  is no ambiguous All Fields mode and no fuzzy row matching.
- Details render the syntax-colored AgentID, typed Agent metadata, then Agent
  Type metadata. JSON is highlighted and scrollable.
- Enter opens a full Agent route. Explicit actions hand one Agent or the
  included set to Oplog, Stream, or Metrics.
- Agent Type metadata is cached by environment, component revision, and Agent
  Type and invalidated by context or deployment revision changes.

## Oplog

- The destination is named Oplog, matching the CLI.
- `[Grouped]` is the default representation; Entries is the exact raw public
  entry view. Newest-first is default and can switch to chronological order.
- Grouping is based only on typed relationships: invocation boundaries,
  start/end or cancellation links, spans, atomic/transaction regions, and
  durable-stream sessions. Ambiguous entries stay separate.
- AgentID and per-Agent oplog index are durable identity. Timestamp order in a
  multi-Agent merge is presentation, not a global durability claim.
- Scope is one exact Agent or an explicit set of at most 20 Agents. Select All
  means the visible loaded candidates subject to that cap, never all backend
  matches.
- Exact text/time search auto-chains bounded requests with concurrency four,
  visible per-Agent index progress, Stop/Resume, and retained continuations.
- A query fills one bounded result batch, then pauses until Load More.
- Grouped and Entries both use a master/details explorer. The Grouped detail
  preserves the typed durable relationship and exposes every constituent exact
  payload; the Entries detail exposes one exact public oplog entry.
- Journal selection and detail reading have independent focus and scroll state.
  The shared Ops split can be resized, placed right or bottom, or hidden; when
  the terminal cannot fit both panes, the list remains usable by itself.

## Stream

- Stream requires one exact Agent and renders typed stdout, stderr, log,
  invocation, plugin, recovery, lag, and error events.
- A connection remains alive across destination switches and records unread
  state. It stops on explicit disconnect, retarget, context change, or exit.
- The buffer is bounded by both 10,000 events and 16 MiB.
- Server-reported missed events and locally dropped buffered events are shown
  separately.
- Connection state is Connected, Reconnecting, or Failed with capped
  exponential backoff and explicit disconnect.

## Metrics

- Internal controls are `[Dashboard | Explore]` and `[Agents | Server]`,
  followed by Scope, Time, Live/Paused, and Refresh.
- First entry defaults to Agent Dashboard, environment scope, and 15 minutes.
  Presets are 5m, 15m, 1h, 6h, 24h, and 7d plus custom absolute from/to.
- An Agent handoff retains the exact selected Agent or included set until the
  user explicitly returns Metrics to environment scope. Switching to Server
  metrics leaves that Agent scope dormant rather than applying it to server
  series.
- Dashboard panels form a scrollable responsive grid: stats plus two columns
  when wide, one panel per row at medium width, and one focused panel when
  narrow.
- Charts expose axes, units, stable legends, gaps, freshness, and
  exact/approximate status. Focus supports keyboard/mouse value inspection and
  an expanded route. In-chart zoom is deferred.

### Agent dashboard

- Stats: Invocations started, Error events, Completed duration p95, Observed
  Agents.
- Panels: invocation rate; completed-duration p50/p95; reliability events by
  kind; top Agent Types/Agents; observed last-known memory; active resources.
- The exporter does not provide one authoritative invocation outcome series,
  so the UI does not invent success rate.

### Server dashboard

- Stats: request rate, failure percentage, request p95, active workers.
- Panels: success/failure throughput; latency p50/p95; active HTTP/gRPC streams;
  worker states and active invocations; memory capacity; admission pressure;
  compilation; cache effectiveness; scheduler/oplog/durable-stream health.
- CPU is not emitted by the local server and is not fabricated.

### Explore

- Families and series are paged and searchable. At most six compatible series
  may be overlaid.
- Delta counters use sums/rates; cumulative counters use reset-aware deltas;
  gauges use last-known values; event families expose count/quantiles; server
  histograms use reset-aware bucket deltas.
- Arbitrary ingested numeric OTLP sum/gauge families are Explore-ready. A guest
  custom-metrics SDK API is not part of this milestone.

## Deployments

- Landing shows the current deployment and recent revisions with Open All
  History.
- Route hierarchy is current deployment → history → revision → compare.
- Revision details contain components, Agent Types, HTTP and MCP deployments,
  remote/published tools, and middleware.
- Compare defaults to selected revision versus its predecessor and renders
  typed added, changed, and removed sections with base/target selectors.
- The API does not expose revision creation time; the TUI does not invent it.

## Configuration

Configuration uses `[Environment | Account | Server]`. Each scope remembers its
own route and selection and loads a kind only when opened.

- Environment: secrets, retry policies, quota resource definitions, domains,
  security schemes, plugin grants, tool grants, and middleware grants.
- Account: identity/roles, current and historical usage, effective limits and
  overrides, owned/received permission shares, API-token metadata, owned
  plugins, tool releases, middleware releases, and cards.
- Server: resolved localServer manifest and source, active nested-session
  overrides when known, connected endpoint, ports, data/filesystem paths,
  memory settings, observability capability, and retention.

Every item uses a typed projection. Secret values and token secrets are never
requested or displayed. Process-local values of an externally started server
remain explicitly unavailable.

## Deferred

- Remote/cloud metrics providers.
- Local trace or log storage; Oplog and Stream cover textual investigation.
- Guest APIs for user-defined metrics.
- Persisted Oplog timestamp indexes.
- Persistent metric rollups and quantile sketches.
- Ops-side mutations, Agent control, deployment execution, rollback, and
  Configuration CRUD.
- Grafana provisioning or a separate web UI.
