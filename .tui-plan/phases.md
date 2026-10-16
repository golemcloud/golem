# Implementation Checkpoints

Each checkpoint must compile, pass focused tests, and form a meaningful commit.

## 1. Lock the contract

- Replace the retired subject/view plan with stable Ops destinations.
- Record page contracts, protocols, deferred work, and acceptance coverage.

Gate: the implementation requires no product decisions not recorded here.

## 2. Stable Ops foundation

- Introduce `OpsApp`, destination routes, tab bar, per-page state, and shared
  collection/async/details/time/chart primitives.
- Move Ops input/render/help/action ownership out of the shell.
- Migrate Agents without a visual or behavioral regression.
- Register deterministic roots for all destinations.

Gate: all destinations are directly navigable and safe at extreme sizes.

## 3. Metrics data foundation

- Upgrade the local store to schema version 2.
- Preserve counter temporality, add source oplog anchors, and implement bounded
  SQL paging/bucketing plus typed Agent/Server dashboard snapshots.
- Carry the OTLP exporter source-index change as the next 1.6.0 source, while
  provisioning the released 1.5.7 plugin and locked 0.0.6 artifact until a
  separate publication checkpoint. Built WASM is a local validation artifact,
  not a committed repository binary.

Gate: typed backend tests prove truthful aggregation, gaps, resets, anchors,
freshness, and approximation metadata.

## 4. Metrics UI

- Implement Dashboard/Explore and Agent/Server controls.
- Add responsive panels, legends, axes, cursor inspection, expansion, time
  ranges, live/pause, refresh, paging, and unavailable states.

Gate: local demo data yields useful Agent and Server dashboards without fake or
mislabelled values.

## 5. Oplog and Stream

- Add bounded structured Oplog search through executor, service, REST, client,
  CLI, and TUI.
- Add shared typed presentation, grouping, multi-Agent progress, Stop/Resume,
  and metric correlation.
- Extract the typed Stream transport and add the persistent TUI session.

Gate: exact index identity survives grouping/merge and sustained stream output
does not starve input.

## 6. Deployments and Configuration

- Add current deployment, history, revision, and compare routes.
- Add lazy typed Environment, Account, and Server configuration inventories.

Gate: sensitive values cannot render and every unavailable scope explains why.

## 7. Overview and acceptance

- Wire the cheap retained-status cockpit, stable quick-jump directory, and
  explicit handoffs without querying Metrics on entry.
- Complete command palette, help, mouse, compact, preview, and documentation
  coverage.
- Install the matching dev-release CLI and exercise the local demo.

Gate: automated matrix and final terminal review are clean.
