---
name: golem-go-development
description: "Builds, tests, and lints the Golem Go SDK modules (core, golem, bridge) and explains their traps: native linking of WIT bindings, the patched Go toolchain, canonical JSON versus the wire form, error shapes, and release tags. Use for changes under sdks/go or the Go bridge generators."
---

# Golem Go SDK Development

## Overview

Work from `sdks/go/<module>/`; the root Rust build does not build the Go SDK. Three
independently built Go modules, none of them part of the root Cargo workspace:

- `core/` — the shared model. `core/schema` holds the schema model, canonical JSON,
  JSON Schema rendering, validation and the schema-native REST wire form;
  `core/values` holds the Go vocabulary for schema values — `Option`, `Result`,
  `TupleN`, `Text`, `Binary`, `Path`, `URL`, `Quantity`. Standard library only.
- `golem/` — the guest SDK: agents, tools, RPC, durability, host wrappers. Requires
  `core`, and holds the generated WIT bindings under `internal/wit`.
- `bridge/` — the runtime generated bridge clients use to call an external Golem
  server: REST for plain calls, the invocation-session WebSocket protocol for
  streaming ones (`session*.go`, checked against the shared vectors in
  `golem-client/tests/fixtures/stream-session-v1/`). Requires `core` and
  `github.com/coder/websocket`, and deliberately not the guest SDK. It also
  holds the leaf conversions and checked accessors (`codec*.go`) the generated
  per-type `encodeX`/`decodeX` functions are composed from, so the generator
  (`cli/golem-cli/src/bridge_gen/go/external.rs`) never spells a wire encoding
  itself. A new helper a generated client calls is public API: change the
  generator and `cli/golem-cli/tests/bridge_gen/go.rs` in the same commit.

`core` exists so there is exactly one implementation of canonical JSON in Go, and so
an external program does not pull in the guest SDK's WebAssembly bindings to speak the
wire format. GOL-653 records what happens when two SDKs each write their own.

`core/values` is shared for a different reason: a guest agent and an external client
generated from the same schema get the *same* Go types, so one domain package serves
both sides. `golem.Option[T]` is an alias — it **is** `values.Option[T]`, not a copy.
Go has no alias for a function, so the constructors (`golem.Some`, `golem.Ok`, …)
forward in one line each.

The value types are built through constructors and keep their fields unexported, so an
inconsistent value cannot be constructed. A reflective codec reaches into them through
sealed interfaces in `core/values`, whose methods are unexported — which means another
module can assert against the interface but cannot call through it. `core/values`
therefore exposes the plumbing as package-level functions (`OptionGet`, `ResultSetOk`,
`QuantityParts`, …) rather than as methods, so a reader of `golem.Option` sees
`IsSome`, `Get` and `Unwrap` and not four reflect-flavoured methods they must never
call.

## Error shapes

A function returns an `error` only when the caller can act on the failure. A failure the
caller can never handle — a host op whose WIT has no error channel, an infrastructure
failure the host has already retried, an SDK invariant, a programming mistake — panics,
which traps the component. Where the error is handleable but callers usually want to
trap, the function has a `Must` variant next to the error-returning form
(`MethodDef.MustCall`, `blobstore.MustGetContainer`); everywhere else `golem.Must`,
`Must0` and `Must2` turn an error into a panic at the call site.

Error kinds are named after the host. A kind constant spells the WIT case name in Go case,
suffix included (`query-execution-failure` is `QueryExecutionFailure`, `custom-error` is
`AgentCustomError`), so code, oplog and docs use one name. A package with one kind type
uses bare names (`postgres.QueryExecutionFailure`, `durablestreams.NotFound`); a package
with several kind types prefixes each with its type's stem (`AgentInvalidInput`,
`RemoteProtocolError`). A kind that only the SDK knows (`tool.CallErrorKind`) follows the
same prefix rule. A switch over a host error variant is pinned and has no catch-all kind.

Generated clients follow where they run. A guest client runs in a component, so it has
the forms of a same-component `MethodDef` — call, trigger, schedule, async — each with
its `Must` variant, and `MustGet<Agent>` beside `Get<Agent>`. An external client runs in
an ordinary Go program, where a panic is not a trap, so it returns errors only.

A Go identifier becomes a schema name in the lowerCamelCase the other SDKs use, with each
initialism as one word (`engine.SchemaName`: `ID` is `id`, `APIKey` is `apiKey`, `UserID`
is `userId`, `UserIDs` is `userIds`). Schemas are compared by field name (a secret shared
by agents in two languages, a tool middleware edge), so `OwnerAgentID` in Go and
`ownerAgentId` in TS or Rust must be the same field. The generators go the other way with
golint's list of initialisms (`cli/golem-cli/src/bridge_gen/go/go.rs`): `owner-agent-id`
and `ownerAgentId` are `OwnerAgentID`, which maps back to `ownerAgentId`, and an agent's
id struct is `<Agent>ID`.

## Prerequisites

- Stock Go matching `golem/go.mod` for `go build` and `go test`.
- The patched `golemcloud/go` fork to build a *component*. The CLI installs and pins
  it; see [Toolchain traps](#toolchain-traps).

## Building and testing

Each module is built on its own:

```shell
cd sdks/go/core   && go build ./... && go vet ./... && go test ./...
cd sdks/go/bridge && go build ./... && go vet ./... && go test ./...
cd sdks/go/golem  && go build ./... && go test ./... \
  && GOOS=wasip1 GOARCH=wasm go build ./... \
  && GOOS=wasip1 GOARCH=wasm go vet -unsafeptr=false -composites=false .
```

The guest SDK is vetted for wasm at `.` only: the generated `internal/wit` bindings trip
vet's `unsafe.Pointer` and unkeyed-field checks, so they are compiled, not vetted. CI runs
this set, the binding drift check and the linters in the `build-golem-go` job
(`.github/workflows/ci.yaml`).

Generated binding code lands in `internal/wit/` and is wiped on every run. The
hand-written export slots in `internal/exports/` survive because `--export-pkg-name`
points the generated glue at them, and `dev-tools/go-bindgen-fixup` works around two
wit-bindgen `crates/go` bugs (tag-constant collisions, and a missing `empty.s` for the
exports package).

Regenerate the committed WIT bindings after a WIT change, and commit the result — CI
rejects drift:

```shell
cargo make generate-sdk-go-bindings
git diff --exit-code sdks/go/golem/internal/wit
```

Changes that reach a running component are verified by building the test component and
running the executor suite:

```shell
cd test-components && ./build-components.sh go
cargo test -p golem-worker-executor --test integration -- --skip diag_ agent_sdk_go
```

Lint with `golangci-lint` per module; each has its own `.golangci.yml`. The version
must be one built with Go 1.27 or newer, or it cannot parse the guest SDK's generic
methods.

## Module paths and directory names

The guest SDK is `github.com/golemcloud/golem/sdks/go/golem`, package `golem`. Go binds an
import to the package clause, but tooling and readers expect it to match the last path
element, so the directory is named `golem` and agents import the SDK with no alias. A
hyphenated name (`golem-go-sdk`) would force an alias back. `core` and `bridge` follow the
same rule.

`componentize-go` is pinned per module through Go's `tool` directive, never installed
globally.

## Native linking and `//go:wasmimport`

**Natively-linked code may import the generated binding packages, but must never make
a host import reachable.**

`empty.s` in each generated package permits a bodyless declaration; it does not define
the symbol. Referencing one from code the native linker keeps gives
`relocation target wasm_import_… not defined` — and the failure is in `go test`, not
`go build`, so a green build says nothing.

Three ways to trip it, all found the hard way:

1. **Binding a generated resource to an interface.** Storing a
   `*types.SchemaValueStream` in an interface materializes its `Drop` method, which is
   a host call. This is why the handle adapters in `internal/witschema/handles_*.go`
   are build-tagged.
2. **An interface method call whose type is structurally matched by a generated type.**
   Calling through an interface makes the linker retain the method sets of everything
   with a matching shape, and `*StreamReader[SchemaValueTree]` lives in an untagged
   generated package.
3. **Generic instantiation waking a dead switch arm.** A type-switch arm pruned as
   dead becomes live the moment some code instantiates the generic type it names,
   dragging its host calls in with it.

**The pattern that works:** keep all logic in an untagged file behind narrow
function-struct indirections — a struct of `func` fields, never an interface — and put
*only* the constructors that touch generated resources in `*_wasm.go` / `*_other.go`.
See `toolstream*.go`, `agentstream*.go`, `reflection*.go`.

Tests are linked code too: a native test that calls a host-backed function — reading
config, for instance — fails to link even though the package builds. Test the pure part
on its own (`checkRouterScope` beside `HTTPRouter.Config`). Streams are the exception
worth knowing: off wasm, `agentstream_other.go` makes a stream pair an in-memory pipe,
so code that produces and consumes `AgentStream`s runs in native tests.

Running `go test` natively in every module is what catches a regression here.

## Go switches are not exhaustive

A `switch` over a WIT tag still compiles when the WIT gains a case; the new case falls
into `default`, or into nothing. A value then converts to `nil`, or a schema node
renders as "unsupported", at runtime, in production, with no compile-time signal.

Where a switch covers a closed sum, pin how many cases it has and enumerate them in a
test, so adding a case to the model fails a test rather than landing in the
"unsupported" arm. The established shape is `witBodyTagCount` in
`golem/internal/witschema/body.go` and `wireValueKinds` / `wireTypeKinds` in
`core/schema/wirejson.go`.

## Canonical JSON and the wire form

Two different JSON encodings cross the boundary and they are not interchangeable:

- **Canonical JSON** (`core/schema/json.go`, `Pack*` / `Unpack*`) renders a value
  *through its schema*, into what an author would write by hand. A record is an object
  with named fields, a s64 is a base-10 string, binary is base64url. The contract is
  written down in `golem-skills/skills/common/golem-agent-reflection`.
- **The schema-native wire form** (`core/schema/wirejson.go`, `Marshal*` /
  `Unmarshal*`) is structural and schema-free: `{"kind": …, "value": …}`, a record is a
  positional list of tagged nodes, a s64 is a number, binary is an array of byte
  numbers. It is the serde shape of the server's Rust types.

Agent configuration overrides are canonical JSON; method parameters and results are the
wire form. Sending one where the other belongs is accepted by neither side.

## Toolchain traps

- **`go version` disagrees with itself.** At the repository root it reports the host
  Go; inside `sdks/go/golem` the `toolchain` directive re-execs the pinned stock
  release from the module cache. Stock Go parses generic methods, so plain `go build`
  of SDK code works — but building a *component* needs the patched `golemcloud/go`
  fork. Anything that shells out to `go` must resolve the toolchain deliberately
  through `cli/golem-cli/src/app/build/go_toolchain.rs`, never `Command::new("go")`, or
  it behaves differently in CI and locally.
- **`gofmt` on `PATH` is the host's**, and an older one cannot parse generic methods,
  so it reports spurious errors. Use `$(go env GOROOT)/bin/gofmt`.
- **`go build ./...` on a component fails natively** with
  `invalid reference to runtime.sbrk`. That is expected — a component is a wasip1
  artifact — but the error says nothing useful. Use
  `GOOS=wasip1 GOARCH=wasm go vet ./...` to type-check one.
- **The toolchain download is checksummed.** When bumping `go_toolchain::TAG`, replace
  the five sha256 values in `go_toolchain::ARCHIVES` (`cli/golem-cli/src/versions.rs`)
  with the new release's `.sha256` assets
  (`gh release download <tag> --repo golemcloud/go --pattern '*.sha256'`); the CLI
  refuses an archive whose hash does not match.
- **`golem-temp/extracted-component-metadata/*.json` can be stale.** A build that
  rebuilt the wasm does not always refresh it, so it can give a confidently wrong
  answer about which agents a component publishes. Check the binary.

## Timers and the async runtime

Go timers (`time.Sleep`, `time.After`, context deadlines) are not something the host
can wait on. An async task whose goroutines wait only on a timer used to exit without
its result, or stay suspended until an unrelated host call completed. Two forks fix
it together, and both are pinned by the CLI:

- [golemcloud/go](https://github.com/golemcloud/go) (patch 4) exposes the next timer's
  due time to the idle callback as `runtime.wasiIdleTimer`;
- [golemcloud/go-pkg](https://github.com/golemcloud/go-pkg), the bindings' async
  runtime, arms a `wasi:clocks/monotonic-clock@0.3.0` `wait-for` until then, so the
  world must import that interface.

Every Go component replaces `go.bytecodealliance.org/pkg` with the fork in its own
`go.mod` (a `replace` in the SDK's `go.mod` does not reach it); the CLI's build check
adds the directive (`versions::go_dep::GO_PKG_FORK`). `go_agent_ops.rs` exercises a
sleeping handler, a timeout racing an RPC, and a deadline on a promise.

## Dependencies

Each module manages its own dependencies in its own `go.mod`; the root Cargo
workspace's central dependency rule does not apply here.

**`core` and `golem` take no third-party dependency.** `core` is standard library
only, and `golem`'s only non-generated dependency is the pinned `componentize-go`
build tool. `bridge` takes one, because it has to: external streaming needs a
WebSocket client and Go's standard library has none, where Scala gets one from
`java.net.http`. It uses `github.com/coder/websocket`, which has no dependencies of
its own; the TypeScript bridge package does the same with `ws`.
A dependency added there must not be reachable from `core` or `golem`.

A `replace` in a dependency's `go.mod` is ignored, so a component that resolves the
guest SDK from a checkout must name `core` itself as well. The CLI does this: it
derives the core path from the Go SDK path override and reconciles both the `require`
and the `replace` on every build (`app/build/check/go.rs`, `sdk_overrides.rs`,
`app/edit/go_mod.rs`). Adding a fourth module means extending that fan-out, and every
component `go.mod` with it.

## Local SDK overrides

`GOLEM_GO_PATH` (or `GOLEM_PATH`, which derives it) makes the CLI write `replace`
directives into a generated app's `go.mod` that point at this checkout. This is how a
playground app tests SDK changes without a release.

## Releasing: Go has no package registry

The git tag is the release; `proxy.golang.org` reads it from this repository, and there is
no publish workflow. A module in a subdirectory must be tagged with that subdirectory as
the prefix, which is a Go rule (<https://go.dev/ref/mod>), so the tags break the
`golem-<lang>-v*` convention of the other SDKs:

```
sdks/go/golem/v0.1.0   ✅ the only form Go recognises
golem-go-v0.1.0        ❌ invisible to Go
```

Each module is tagged on its own (`sdks/go/core/…`, `sdks/go/golem/…`,
`sdks/go/bridge/…`). The tags collide with nothing: there is no root `go.mod`, so the
repository's own release tags are invisible to Go. With the default `GOPROXY` a consumer
downloads only the module subtree. The mirror keeps a module permanently only if it detects
a license, and Go has no metadata field for one, so each module directory carries its own
`LICENSE`.

The three modules release in lockstep at one version: a generated app requires the guest SDK
and core, and a generated external client requires the bridge and core, all at
`versions::sdk::GO`. To release version `X`:

1. In `sdks/go/golem/go.mod` and `sdks/go/bridge/go.mod`, change `require …/sdks/go/core
   v0.0.0` to `vX`. The `replace … => ../core` stays for in-repo builds, but a consumer
   ignores a dependency's `replace`, so a published module that requires `v0.0.0` cannot be
   resolved.
2. Set `versions::sdk::GO` in `cli/golem-cli/src/versions.rs` to `X`.
3. Tag the same commit `sdks/go/core/vX`, `sdks/go/golem/vX` and `sdks/go/bridge/vX`, and
   push the three tags together.
4. Check that the proxy resolves each module, for example
   `GOPROXY=https://proxy.golang.org go list -m github.com/golemcloud/golem/sdks/go/golem@vX`,
   and that pkg.go.dev shows the license.

Two pins in `versions.rs` are released elsewhere and only change when their own release
changes: `go_toolchain::TAG` (the `golemcloud/go` fork) and `go_dep::GO_PKG_FORK` (the
`golemcloud/go-pkg` fork). The CLI writes the `go-pkg` replace into every component's
`go.mod`, because the replace in the SDK's own `go.mod` does not reach them, so a change to
the SDK's `go-pkg` replace needs the same change in `GO_PKG_FORK`.
