# Ops Rebuild Tasks

`[ ]` pending, `[~]` active, `[x]` complete.

## Contract

- [x] Stable destination and navigation model locked.
- [x] Metrics dashboard, Explore, and panel catalogue locked.
- [x] Oplog bounded-search and correlation direction locked.
- [x] Configuration Environment/Account/Server scope locked.
- [x] TUI-only first milestone and deferred work recorded.

## Foundation

- [ ] Add typed `OpsDestination` and per-destination route stacks.
- [ ] Add horizontal scrollable Ops tab bar and direct leader navigation.
- [ ] Introduce `OpsApp` and delegate render/input/help/actions from the shell.
- [ ] Add shared async, collection, details, time-range, and chart state.
- [ ] Migrate Agents and metadata caching to the new boundary.
- [ ] Derive help and palette visibility from exact route/focus.

## Metrics backend

- [ ] Add temporality and source oplog index to local metric models/store.
- [ ] Upgrade local observability schema to version 2.
- [ ] Replace whole-range in-memory queries and every-Nth sampling.
- [ ] Add truthful bucket transforms, gauge seeds, gaps, and approximation flags.
- [ ] Add typed Agent/Server dashboard request and response.
- [ ] Add exporter source-index attributes and release version 1.6.0.
- [ ] Rebuild, validate, and commit the exporter WASM.

## Metrics UI

- [ ] Add Dashboard/Explore and Agent/Server controls.
- [ ] Add global range, live/pause, refresh, and freshness state.
- [ ] Implement Agent stats and six curated panels.
- [ ] Implement Server stats and nine curated panels.
- [ ] Add responsive chart grid, cursor inspection, expansion, and handoff.
- [ ] Add paged Explore families/series and six-series compatibility checks.

## Oplog and Stream

- [ ] Add executor scan-limit configuration and generated config updates.
- [ ] Separate raw GET paging from structured POST search.
- [ ] Carry scanned progress and continuations across gRPC/service/REST/client.
- [ ] Add CLI time-range search flags and preserve structured entry output.
- [ ] Extract typed Oplog presentation and implement safe grouping.
- [ ] Implement exact/mixed scope, order, Stop/Resume, and Load More.
- [ ] Extract typed Stream transport from CLI output.
- [ ] Add persistent background Stream buffer, unread, reconnect, and drop state.

## Deployments and Configuration

- [ ] Add current deployment and recent revision landing.
- [ ] Add full history, revision details, and typed compare.
- [ ] Add Environment configuration kinds with typed masked projections.
- [ ] Add full Account catalogue with typed projections.
- [ ] Add resolved/observed Server configuration and capability kinds.

## Overview and acceptance

- [ ] Add deployment, Agent signal, server/collector, Stream, and issue cards.
- [ ] Add explicit cross-destination scope/range handoff.
- [ ] Add all page/state preview fixtures behind `tui-preview`.
- [ ] Complete wide/medium/narrow/short/1×1 automated render coverage.
- [ ] Complete focused tests, checks, generated artifacts, and manual demo run.
- [ ] Update review ledger and mark the Ops foundation accepted.
