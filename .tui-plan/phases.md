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

## 5. Dev/Ops Workspace Structure

Acceptance criteria:

- dev and ops actions are categorized and surfaced intentionally;
- context help and palette can distinguish dev and ops workflows;
- component/environment/resource views can be added without overloading global shortcuts.

## 6. Local Observability For Issue #3456

Acceptance criteria:

- the TUI has a clear role in the local OTel metrics experience;
- baseline stack and POC direction are documented;
- the TUI can launch, monitor, or link to the chosen local observability workflow as appropriate.
