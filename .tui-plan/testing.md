# TUI Validation Plan

## Unit and State Tests

- Destination switching, tab scrolling, route push/pop, state restoration, and
  context resets.
- Collection filter/find distinction, cursor/end state, selection visibility,
  literal match highlighting, and explicit server-search escalation.
- Details placement, four-direction resize, scrollbar ownership, mouse hit
  regions, and route-derived actions/help.
- Dashboard transforms: delta and cumulative counters, counter reset, gauge
  seed, histogram quantiles, event quantiles, gaps, units, freshness, and
  exact/approximate metadata.
- Oplog scan limit, empty-match continuation, timestamp inclusivity, component
  revision tracking, typed grouping, cancellation, and multi-Agent merge.
- Stream reconnect, cancellation, background unread state, ring eviction, and
  separate server/local loss counters.
- Secret, token, and security projections prove sensitive fields cannot render.

## Service and Artifact Tests

- Local observability schema-version rejection and schema-2 round trips.
- Families/series/sample SQL paging and dashboard query tests.
- Exporter processing tests prove first-entry-index propagation and reserved
  attribute stripping from series identity.
- Worker executor/service tests cover the structured search request and scanned
  progress through archived/multilayer storage.
- REST endpoint and generated client tests cover request validation,
  authorization, and continuation with zero matches.

Required generation and artifact checks:

```shell
cargo make generate-configs
cargo make generate-openapi
cargo build -p golem-client
cargo make build-plugins
wasm-tools validate --features all plugins/otlp-exporter.wasm
```

`build-plugins` may materialize the ignored exporter WASM for local validation.
The released artifact remains identified by `builtin-artifacts.lock.json`; the
generated binary is not committed.

Run focused package tests while iterating, then the affected service/client
checks, `cargo check -p golem-cli --features tui-preview`, registry-service
check, and authoritative built-in-plugin integration group. Never use
`cargo make test`.

## Render Matrix

Every destination covers ready plus relevant loading, refreshing, empty,
partial, unavailable, stale, and failed states at:

- wide: 140×40;
- medium: 100×30;
- narrow: 60×24;
- short: 100×12;
- extreme: 1×1.

Assertions include no panic/overflow, visible focus, content ending before its
right scrollbar, draggable dividers, stable popup size while filtering, and no
footer/help duplication.

Preview fake telemetry is feature-gated and visibly labelled. Production
rendering uses only live providers.

## Manual Acceptance

- Install the matching `golem` dev-release binary from this checkout.
- Use the matching `golem-00` playground, custom ports, and an isolated data
  directory.
- Exercise few, hundreds, and thousands of Agents from the demo population
  scripts.
- Verify Dashboard, Explore, exact and mixed Oplog, background Stream, revision
  navigation, all Configuration scopes, mouse scrolling/dragging, compact
  layout, command palette, and Dev↔Ops restoration.
- Finish with package formatting checks and `git diff --check`.
