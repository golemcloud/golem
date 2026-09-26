# Golem Go SDK

## Overview

Three independently built Go modules, none of them part of the root Cargo workspace:

- `core/` — the shared model. `core/schema` holds the schema model, canonical JSON,
  JSON Schema rendering, validation and the schema-native REST wire form;
  `core/values` holds the Go vocabulary for schema values — `Option`, `Result`,
  `TupleN`, `Text`, `Binary`, `Path`, `URL`, `Quantity`. Standard library only.
- `golem/` — the guest SDK: agents, tools, RPC, durability, host wrappers. Requires
  `core`, and holds the generated WIT bindings under `internal/wit`.
- `bridge/` — the runtime generated bridge clients use to call an external Golem
  server over REST. Requires `core`, and deliberately not the guest SDK. It also
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
- **`golem-temp/extracted-component-metadata/*.json` can be stale.** A build that
  rebuilt the wasm does not always refresh it, so it can give a confidently wrong
  answer about which agents a component publishes. Check the binary.

## Dependencies

Each module manages its own dependencies in its own `go.mod`; the root Cargo
workspace's central dependency rule does not apply here.

**`core` and `golem` take no third-party dependency.** `core` is standard library
only, and `golem`'s only non-generated dependency is the pinned `componentize-go`
build tool. `bridge` may take one, because it has to: external streaming needs a
WebSocket client and Go's standard library has none, where Scala gets one from
`java.net.http`. The TypeScript bridge package does the same, depending on `ws`.
A dependency added there must not be reachable from `core` or `golem`.

A `replace` in a dependency's `go.mod` is ignored, so a component that resolves the
guest SDK from a checkout must name `core` itself as well. The CLI does this: it
derives the core path from the Go SDK path override and reconciles both the `require`
and the `replace` on every build (`app/build/check/go.rs`, `sdk_overrides.rs`,
`app/edit/go_mod.rs`). Adding a fourth module means extending that fan-out, and every
component `go.mod` with it.
