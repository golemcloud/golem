# Ops Rebuild Tasks

`[ ]` pending, `[~]` active, `[x]` complete.

## Contract

- [x] Stable destination and navigation model locked.
- [x] Metrics dashboard, Explore, and panel catalogue locked.
- [x] Oplog bounded-search and correlation direction locked.
- [x] Configuration Environment/Account/Server scope locked.
- [x] TUI-only first milestone and deferred work recorded.

## Foundation

- [x] Add typed `OpsDestination` and per-destination route stacks.
- [x] Add horizontal scrollable Ops tab bar and direct leader navigation.
- [ ] Introduce `OpsApp` and delegate render/input/help/actions from the shell.
- [~] Add shared async, collection, details, time-range, and chart state.
- [ ] Migrate Agents and metadata caching to the new boundary.
- [ ] Derive help and palette visibility from exact route/focus.

## Metrics backend

- [x] Add temporality and source oplog index to local metric models/store.
- [x] Upgrade local observability schema to version 2.
- [x] Replace whole-range in-memory queries and every-Nth sampling.
- [x] Add truthful bucket transforms, gauge seeds, gaps, and approximation flags.
- [x] Add typed Agent/Server dashboard request and response.
- [x] Add exporter source-index attributes and release version 1.6.0.
- [x] Rebuild, validate, and commit the exporter WASM.

## Metrics UI

- [x] Add Dashboard/Explore and Agent/Server controls.
- [x] Add global range, live/pause, refresh, and freshness state.
- [x] Implement Agent stats and six curated panels.
- [x] Implement Server stats and nine curated panels.
- [x] Add responsive chart grid, cursor inspection, expansion, and handoff.
- [x] Add paged Explore families/series and six-series compatibility checks.

## Oplog and Stream

- [x] Add executor scan-limit configuration and generated config updates.
- [x] Separate raw GET paging from structured POST search.
- [x] Carry scanned progress and continuations across gRPC/service/REST/client.
- [x] Add CLI time-range search flags and preserve structured entry output.
- [x] Extract typed Oplog presentation and implement safe grouping.
- [x] Implement exact/mixed scope, order, Stop/Resume, and Load More.
- [x] Extract typed Stream transport from CLI output.
- [x] Add persistent background Stream buffer, unread, reconnect, and drop state.

## Deployments and Configuration

- [x] Add current deployment and recent revision landing.
- [x] Add full history, revision details, and typed compare.
- [x] Add Environment configuration kinds with typed masked projections.
- [x] Add full Account catalogue with typed projections.
- [x] Add resolved/observed Server configuration and capability kinds.

## Overview and acceptance

- [x] Add lightweight deployment/server/collector/Stream status and a stable
  quick-jump directory without querying Metrics or full revision composition.
- [x] Add explicit cross-destination scope/range handoff.
- [~] Add all page/state preview fixtures behind `tui-preview` (all ready-state destinations complete; exceptional and nested route states remain).
- [x] Complete wide/medium/narrow/short/1×1 automated render coverage.
- [ ] Complete focused tests, checks, generated artifacts, and manual demo run.
- [ ] Update review ledger and mark the Ops foundation accepted.

## Ops UX consolidation

- [x] Give every active text input explicit mode ownership, shared input
  styling, a visible terminal cursor, and apply/cancel semantics.
- [x] Replace the query-heavy Overview dashboard with a cheap status and
  quick-jump cockpit.
- [x] Apply one hierarchy and continuous pane chrome across Ops destinations.
- [x] Make Agent, Metrics, Oplog, and Stream scope handoffs visible in-page.
- [x] Rebuild Oplog as a navigable master/details journal and forensic viewer.
- [x] Bring row selection, pane scrolling, split resizing, and scrollbar
  interaction to pointer parity on every Ops destination.
- [ ] Review Metrics chart density, legends, inspection, and empty/error states
  as a final minimal-Grafana pass.
- [ ] Complete terminal visual review of each finished checkpoint before Ops
  acceptance.
