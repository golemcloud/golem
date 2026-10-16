# TUI Architecture

## Runtime Boundary

The runtime owns terminal setup and one async event loop over terminal input,
provider completions, nested CLI traffic, persistent sessions, streams,
animation ticks, and refresh timers.

- Terminal and control events have priority over bounded high-volume output.
- Stream and process output apply backpressure and are drained with a per-frame
  budget so redraw traffic cannot starve keyboard or mouse events.
- Replaceable refresh/spinner ticks may be coalesced or dropped.
- Every provider completion carries context identity and request generation;
  stale work is discarded.
- Dev nested commands retain the existing PTY boundary. Ops calls typed
  providers and never invokes another CLI process.

## Application State

`TuiApp` owns shell, Dev, overlay, Job, and context state plus one `OpsApp`.
`OpsApp` owns:

- active stable destination and horizontal tab viewport;
- a route stack and session state for each destination;
- explicit cross-destination scope handoffs;
- page-specific async requests and persistent Stream transport;
- reusable collection, details, time-range, notice, and chart state.

The shell delegates Ops input, rendering, actions, help, and geometry to
`OpsApp`; it does not branch on every Ops page. Page modules own their state,
update logic, focus order, render tree, hit regions, and contextual actions.

## Reusable Ops Primitives

- `AsyncValue<T>`: initial load, ready, refreshing-with-stale-value, empty,
  unavailable, partial, and failed.
- `CollectionState<T>`: server cursor, end state, loaded rows, literal Find,
  selection, viewport, and scroll restoration.
- `DetailsState`: visible, focused, right/bottom placement, ratio, and scroll.
- `TimeRangeState`: relative/custom range, live/pause, refresh cadence, and
  exact handoff range.
- `ChartState`: datasets, axes, legend, selected panel/sample, cursor, and
  expansion route.
- `OpsRoute`: typed Overview, Agent, Agent SSH, Oplog, Stream, Metrics,
  Deployment, and Configuration routes; breadcrumbs derive from the route.

Only placement and ratios are preferences. Scope, filters, route depth,
buffers, cursors, and query text are session data. A context change resets the
latter and preserves the former.

Agent scope is transferred only by an explicit action. Oplog retains the exact
one-Agent or included-set route and never adopts a hidden Agent-table
selection when its root is opened. Metrics retains the transferred Agent
scope in its page state while preserving environment scope as an explicit
choice. Stream accepts either the exact full-Agent route or exactly one
included Agent; a mixed set cannot be connected accidentally.

## Providers and Caching

- Reuse neutral data-returning command-handler helpers where CLI and TUI share
  resolution or projection logic. Rendering and logging stay at their edges.
- Direct typed clients are allowed for safe reads that have no neutral handler.
- Cache immutable metadata with its real identity: Agent Type metadata is keyed
  by environment, component revision, and Agent Type.
- Current deployment, account/configuration collections, and observability
  snapshots carry freshness and request keys. They never cross context
  generations.
- Pages load independently; one failed card or kind does not blank its siblings.

## Local Observability

The built-in server samples its Prometheus registry every five seconds and
accepts Agent OTLP/HTTP JSON metrics into a bounded SQLite store.

Local observability is off unless explicitly enabled by the manifest, config,
environment, or CLI. It is a local-server feature: enabling it with a separate
management host or without a well-known local token is a startup error. Existing
deployments are never rewritten at server startup; the exporter overlay is
attached on the next normal application deploy, while already collected data is
retained until explicitly cleaned.

Schema version 2 records:

- source, family, semantic kind, temporality, statistic, timestamp, value;
- environment/component/Agent/Agent-Type scope and stable series key;
- attributes excluding reserved correlation metadata;
- nullable exact source oplog index.

Existing incompatible databases are rejected with the targeted
`golem server clean --observability-only` instruction. No schema migration is
provided.

Explore retains families, series, and samples APIs. A typed dashboard endpoint
returns Golem-specific Agent or Server snapshots rather than a generic
dashboard-definition language. SQL performs filtering, grouping, pagination,
counter deltas, and time bucketing where possible. Event quantiles are exact up
to 100,000 points, then deterministically sampled and marked approximate.

## Oplog Search and Presentation

Raw Oplog reads remain exact index pages. Search is a separate structured
operation with optional text and time predicates, a result limit, a scan limit,
and a continuation cursor.

- Each server request scans at most the configured bound and returns scanned
  index progress even with zero matches.
- Time filtering runs on raw entries before public-entry enrichment while
  component-revision state advances through skipped entries.
- Per-Agent continuations allow the TUI to stop/resume and merge at most 20
  explicit Agents with concurrency four.
- A neutral `OplogEntryPresentation` retains the typed public entry and owner
  attribution beside its JSON projection. TUI code never parses ANSI or
  serialized JSON to infer entry type, timestamp, correlation, or ownership.
- The grouping projection is per Agent and tolerant. Explicit typed keys win;
  contiguous raw pages may additionally pair invocation start/finish markers
  using best available loaded context. Unmatched and overlapping structures
  remain visible with open, partial, or orphan state.
- Sparse server-search results disable adjacency inference. The UI can fetch an
  exact raw page around a selected hit before reconstructing context. A
  separate client filter operates only on retained entries and cannot issue or
  broaden a server query.

## CLI Log And Session Boundaries

The `log` module exposes purpose-specific capabilities: contextual capture,
deferred command messages, raw stdout/stderr reservations, and process-output
lifecycle. Callers do not select an arbitrary output sink across those
boundaries. SSH connection, contract validation, submission, remembered cwd,
and Agent-fingerprint state live in a shared session engine. The standalone
adapter retains Reedline and process-terminal ownership; the Agent-scoped TUI
route owns native editing, history, semantic rendering, cursor, scrolling, and
context generations. Neither terminal runtime is nested inside the other.

The OTLP exporter attaches the exact source index to every metric point emitted
from an oplog entry. Exact one-Agent samples jump directly; aggregate points
carry the server-computed bucket width and transfer that clamped absolute time
range plus their exact one-Agent series scope to bounded search.

## Stream Transport

The Agent websocket transport emits typed events and connection state through
a bounded channel. CLI stdout formatting and TUI ring-buffer storage are
separate consumers. The TUI session owns cancellation, retargeting, capped
exponential reconnect, unread state, and distinct server/local drop counts.

## Feature Boundaries

- Production TUI stays on default CLI builds.
- Preview fixtures, browser host, fake telemetry, and lab-only render helpers
  remain behind `tui-preview` and are visibly labelled.
- Local observability remains separately feature-gated where it is today.
- No fake provider may be reachable from a production context.
