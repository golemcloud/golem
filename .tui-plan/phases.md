# TUI Rebuild Phases

This file is the concise checkpoint map. Its numbers match tasks.md exactly.
A checkpoint starts only after its named planning prerequisites are checked and
ends only after behavior, rendering, focused tests, and its recommended commit
boundary are complete.

Implementation checkpoint `721390b80` completes the accepted local Dev and
read-only Ops subset across checkpoints 1-5 and the truthful-unavailable first
item of checkpoint 7. `tasks.md` is authoritative for partial items and open
gates; `UX-024` owns the remaining real-terminal acceptance pass.

## 0. Contract And Readiness

Deliverables:

- synchronized product, UI, architecture, task, testing, review, and progress
  documents;
- one visible readiness ledger with locked and open decisions;
- no inherited commitment to the previous embedded observability backend.

Gate: the user confirms that the ledger accurately reflects the discussion.

## 1. Product State And Actions

Prerequisites: typed provider/order review and exact resource-action inventory.

Deliverables:

- scalable Dev/Ops workspace, Dev view/layout, and Ops subject/view state;
- selected target descriptor with provenance and per-view capabilities;
- one action registry with hierarchical leader paths, palette relevance,
  availability, target, risk, and review metadata;
- switchable All/Context palettes over the same registry.

Acceptance:

- current Agents behavior remains usable during the transition;
- reducer tests cover product navigation, capability, palette, and stale-context
  transitions;
- replaced flat workspace and shortcut compatibility paths are removed.

## 2. Shared Shell And Recursive Layout

Prerequisite: responsive shell-priority rules.

Deliverables:

- responsive Dev/Ops shell with context, Jobs, Live, and remote-lock status;
- ctrl+x w/v/l/a/j/e hierarchy;
- recursive right/bottom Dev subviews and Ops Details layouts;
- temporary Layout mode, mouse resizing, compact fallback, and persisted visual
  preferences.

Acceptance:

- every pane and overlay renders at wide, short, narrow, compact, and 1x1 sizes;
- tab changes pane focus only and prohibited shortcut families are absent;
- dividers, spines, hit regions, and structural scrollbars never overlap.

## 3. Jobs, Sessions, And Safety

Prerequisites: quit policy and exact action/risk inventory.

Deliverables:

- bounded global Jobs registry and overlay;
- separate Server, REPL, and Live session lifecycles;
- risk-class action policy and per-context remote write unlock;
- context-switch and quit behavior;
- one active mutating Job per environment with no automatic queue.

Acceptance:

- no mutation can bypass target review, remote lock, or risk confirmation;
- active and failed work is globally visible;
- target identity and secrets remain correct across navigation and errors.

## 4. Minimal Dev Workspace

Prerequisite: exact Dev view content, controls, states, and actions. Complete.

Deliverables:

- Build & Deploy with Plan, Build, Deploy, Clean, structured results, and Job
  history projection;
- Server with local lifecycle, process state, bound ports, data directory, and
  logs;
- REPL with explicit input ownership and persistent PTY state;
- optional right/bottom subviews with no-subview first-run default.

Acceptance:

- a user can start a local server, plan/build/deploy, use the REPL, switch to
  Ops, and return without losing session or layout state;
- unavailable per-view capabilities show concrete reasons.

## 5. Ops Resource Explorer

Prerequisite: exact resource-action inventory.

Deliverables:

- root subjects Agents, Deployment, Environment, and Server;
- resource drill-downs and breadcrumbs;
- accepted collection/details behavior on every resource list;
- Deployment history and revision diffs;
- Environment resource catalog and homogeneous collections;
- contextual resource actions using Jobs and safety policy.

Acceptance:

- resource navigation is complete by mouse, leader, and palette;
- server Dataset paging and local Find remain visibly distinct and bounded;
- local and remote mutations follow the same explicit risk contract.

The first implementation pass through this checkpoint is read-only: it adds
the four root subjects, individual Agent Overview, current Deployment identity,
selected Environment configuration, and Server target/capability information.
History, Diff, Resources, deeper drilldowns, and mutations remain gated.

## 6. Activity

Prerequisite: concrete soft/default and hard Agent bounds plus overflow review.

Deliverables:

- direct typed oplog provider;
- bounded multi-Agent scope resolver;
- Timeline and Journal;
- direct single-Agent Live session.

Acceptance:

- Journal never loses AgentID or per-Agent oplog index;
- Timeline ordering is labelled as presentation rather than global causality;
- Live remains visibly bound to one exact Agent and never silently retargets.

## 7. Provider-Independent Metrics

Prerequisite: backend-independent range, aggregation, refresh, auto-refresh,
and follow controls.

Deliverables:

- family then series navigation;
- samples, attributes, details, filters, ranges, refresh, and sparklines;
- distinct Golem service and Agent OTLP source availability;
- preview-only fake provider and truthful production unavailable states.

Acceptance:

- the TUI interaction model is visually accepted without assuming receiver,
  storage, retention, or companion tooling;
- high-cardinality series do not flatten into an unbounded primary table.

## 8. Metrics Acquisition Decision

This checkpoint begins only after checkpoint 7 acceptance. Validate real
payloads and connectivity, compare backend choices against the typed query
contract, and produce a separate implementation plan for GOL-162 acquisition.
Docker, embedded storage, and a custom web UI remain options rather than
assumptions.

## 9. Cleanup And Final Acceptance

Deliverables:

- removed dormant and replaced TUI paths;
- stable module boundaries;
- synchronized action/help metadata;
- focused and complete TUI validation;
- installed-binary terminal smoke test.

Acceptance: product-contract.md, ui-system.md, code, help, previews, and tests
describe the same minimal Golem TUI.
