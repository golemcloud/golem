# Config conformance corpus

`v1.json` contains hand-authored schemas, inputs and expected results. It is not
generated from an SDK, extracted metadata or the resolver. A producer must match
these expectations, not regenerate them to match its output.

## Executable static contract

`schemas` uses the native `SchemaGraph` and `AgentConfigDeclarationSchema` JSON
forms. `default_value` is an optional canonical JSON **string**: an omitted field
differs from the string `"null"`. `Local` is the metadata source for static fields.
Graph definition IDs need not match across producers; field paths, branch tags,
discriminators, types and decoded canonical defaults must match. JSON whitespace
and object-key order are not differences in the config contract.

Each `staticCases` entry selects a schema, an exact-path catalog and ordered
configuration layers, from lowest to highest precedence. Its result is either an
independently specified field map or an error category. Defaults fill absence;
they do not repair supplied invalid values or broken selectors. Record overlays
retain missing fields, discriminator changes replace the previous union branch,
and optional tuple positions retain their indices. Canonical field names do not
recase catalog paths.

The `reference` object supplies the real Rust guest's deployment fill, creation
override and expected `read` result. The raw JSON returned by that guest uses its
Rust serialization for the union (`{"Text":"selected:value"}`); canonical config
JSON uses the prefix discriminator (`"selected:value"`). These are different
representations, not interchangeable expectations.

Build the Rust `agent-sdk-rust` test component with the current SDK and CLI using
its `golem.yaml`; `test-components/build-components.sh` owns the build/copy flow.
The copied output is `test-components/golem_it_agent_sdk_rust_release.wasm`.
From the repository root, run:

```sh
cargo test -p golem-common --test config_conformance -- --report-time
RUST_MIN_STACK=33554432 cargo test -p golem-worker-executor-tests --test integration -- static_config_defaults_and_creation_overrides_survive_reconstruction --report-time
```

The common runner checks all static cases and compares actual guest metadata
with the reference graph using cross-graph type equivalence. The executor test
deploys that component, reads through the guest host API, destroys and recreates
the executor over the same storage, and checks the captured result and settled
oplog. Neither test builds components or launches external tools.

## Source forms are not managed runtime acceptance

`sourceCases` checks only the resolver's source plans. An omitted Shared optional
captures absence; adding a catalog entry alone does not attach it. An explicitly
bound entry containing null remains live. Omission preserves an inherited
binding. A supplied broken selector, anonymous null, wrong catalog kind, missing
Shared list binding or missing Secret binding fails rather than becoming absence.

Those checks do not execute Shared reads, catalog edits, revision updates or
special-field operations. [Later runtime scenarios](managed-and-special.md)
assign those checks to the units that deliver them.

## SDK entrypoints

These are existing local build/test entrypoints, not claims that each SDK has
ported this corpus. Each static SDK delivery must consume the corpus, build an
actual guest and add its host/reconstruction case. Unit mocks and successful WASM
validation alone do not complete that delivery. Follow each SDK's prerequisites
and pinned generators before running its commands.

| SDK / delivery     | Working directory            | Existing entrypoints                                                                                                                                 |
| ------------------ | ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| Rust / TC-17       | `sdks/rust`                  | `cargo test -p golem-rust --lib --test agent`; build `test-components/agent-sdk-rust` through its manifest                                           |
| TypeScript / TC-18 | `sdks/ts`                    | `pnpm --filter @golemcloud/golem-ts-sdk run test`; `pnpm run build`; `pnpm run build-agent-template`                                                 |
| Effect / TC-19     | `sdks/effect`                | `npm test`; `npm run build-agent-template`; `npm run check:artifacts`                                                                                |
| Go / TC-20         | `sdks/go/golem`              | `go test ./...`; `GOOS=wasip1 GOARCH=wasm go build ./...`; build `test-components/agent-sdk-go/module` with the repository-pinned Go fork            |
| Scala / TC-21      | `sdks/scala`                 | `sbt '++3.8.2; golemTestAll'`; `sbt '++3.8.2; testAgents/golemBuildComponent'`; affected Mill consumers also require their own build                 |
| MoonBit / TC-22    | `sdks/moonbit/golem_sdk`     | `bash scripts/run-sdk-tests.sh agents config reflection`; `moon check --target wasm`; build an actual foreign-library guest with the pinned bindings |
| Python / TC-23     | Not present in this checkout | Requires the actual backend and its announced entrypoint; no placeholder runner                                                                      |

The common commands above remain runnable without AI schemas, managed-resource
authorities or special-field suppliers. A failure to extract a portable default
must be an explicit producer error, not silent loss of the default or an SDK-local
fallback at read time.
