# Phases

The original scaffold phases are complete enough that future work should be organized by foundation arcs.

## 1. Action And Help Rules

Acceptance criteria:

- action metadata includes IDs, labels, shortcuts, categories, scopes, and execution kind;
- palette, footer, leader hints, and help use action metadata where practical;
- context help is reachable from focused modes;
- tests catch shortcut/help/palette drift.

## 2. Test Driver And Regression Harness

Acceptance criteria:

- `TuiTestDriver` can send keys, render frames, assert visible text, and dump failure context;
- high-level scenario tests cover the main manual UX paths;
- targeted validation commands are documented and green.

## 3. Scoped Logging And Context Execution

Acceptance criteria:

- direct handler calls can capture logs without mutating global output state for other concurrent work;
- indentation and buffered output are scoped correctly across `.await`;
- spawned TUI work uses an explicit helper to propagate logging context and tracing span;
- the TUI has a context executor that reuses runtime infrastructure, carries context identity/generation, and leaves request/event mapping in the view;
- no direct TUI refresh creates a new Tokio runtime or blocks on it.

## 4. Context And Environment Model

Acceptance criteria:

- selected TUI context is separate from initial CLI `Context`;
- manifest, local, cloud, custom, and non-manifest modes are explicit;
- jobs capture immutable launch context;
- unavailable actions show a reason.

## 5. Ops-First Production Rebuild

Acceptance criteria:

- the accepted Frame Base is the sole production visual system;
- the TUI starts in Ops and renders only `[Ops]` until a rebuilt Dev workflow
  is ready;
- Agents is the first subject, with Overview and Metrics as production views;
- Agents Overview distinguishes server Dataset filtering from local Find,
  loads bounded component cursors incrementally, and never invents totals or
  page numbers;
- reusable list/details blocks expose refresh state, semantic cells, resizable
  focus, independent scrolling, and structured metadata;
- the temporary OTLP explorer uses deterministic samples, labels all panes as
  fake, and explicitly states that it does not read a receiver or query store;
- old Home, Dev, drawer, REPL, output, and nested inspect surfaces are not
  reachable from footer, palette, help, or direct shortcuts;
- component and other entity subjects can be added without renaming Metrics or
  Activity.

## 6. Local Observability For GOL-162

Acceptance criteria:

- the local `golem` process owns a metrics-only OTLP HTTP receiver and bounded
  persistent observability store without requiring Docker, Prometheus, or
  Grafana;
- normal deploy/update may add the local exporter to the effective deployment,
  but startup never revises components and the manifest is never edited;
- the TUI distinguishes server and agent metric availability without
  fabricating data;
- Activity keeps Timeline and Journal modes but remains out of production until
  its direct typed provider supports explicit bounded multi-agent selection.
