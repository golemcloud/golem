# Go SDK — known problems and follow-ups

Things found while building the SDK that are deliberately not fixed yet. Each
says what the problem is, why it was left, and what fixing it would take.

---

## 1. Go type switches are not exhaustive

A `switch` over a WIT tag compiles fine when the WIT gains a case; the new case
just falls into `default` — or into nothing, if there is no default. A value
then converts to `nil`, or a schema node renders as "unsupported", at runtime,
in production, with no compile-time signal.

Handled in `internal/witschema/body.go`: the switch ends in an error naming the
tag, and `witBodyTagCount` pins how many cases the bindings declare, checked by
`TestEveryWitBodyTagConverts`. Adding a WIT case then fails a test rather than
silently losing data.

**The same pattern is still missing** in every other exhaustive switch over
generated tags:

- `codec.go` `buildCodec` / `sdkComposite` — a new schema kind gets no codec.
- `core/schema/json.go` `render` / `build`, `jsonschema.go` `renderBody`.
- `internal/witschema/value.go` — value tags, as opposed to type tags.
- `tooldef.go` / `reflection.go` — tool metadata and command bodies.

Worth applying the pinned-count test to each. The cheap version is one test per
switch; the better version is a single generated table the switches are checked
against.

## 2. Native linking and `//go:wasmimport`

This cost four separate debugging sessions, so it is worth stating plainly.

**Rule:** natively-linked code may *import* the generated binding packages, but
must never make a host import *reachable*. `empty.s` in each generated package
permits a bodyless declaration; it does **not** define the symbol. Referencing
one from code the native linker keeps gives
`relocation target wasm_import_… not defined`, and the failure is in `go test`,
not `go build`.

Three ways to trip it, all found the hard way:

1. **Binding a generated resource to an interface.** Storing a
   `*types.SchemaValueStream` in an interface materialises its `Drop` method,
   which is a host call. This is why the handle adapters in
   `internal/witschema/handles_*.go` are build-tagged.
2. **An interface method call whose type is structurally matched by a generated
   type.** `AgentStream`'s first `treeSource` was an interface; calling through
   it made the linker retain the method sets of everything with a matching
   shape, and `*StreamReader[SchemaValueTree]` lives in an untagged generated
   package. Fixed by making `treeSource`/`treeSink` structs of functions.
3. **Generic instantiation waking a dead switch arm.** `case streamish:` in
   `sdkComposite` was pruned until some code instantiated `AgentStream[T]`;
   then its closures became live and dragged the value-node path in.

**Pattern that works:** keep all logic in an untagged file over narrow
function-struct indirections, and put *only* the constructors that touch
generated resources in `*_wasm.go` / `*_other.go`. See `toolstream*.go`,
`agentstream*.go`, `reflection*.go`.

**Follow-up:** this belongs in the SDK's contributor documentation, not only
here. A CI check that builds the test binary natively would catch regressions,
which `go build` alone does not.

## 3. The authoring codec drops schema restrictions

`codec_aggregate.go` hardcodes the restriction payloads:

| Type | Emitted as |
|---|---|
| `golem.Text` | `TextRestrictions{}` — no language, length or pattern |
| `golem.Binary` | `BinaryRestrictions{}` — no MIME types or size bounds |
| `golem.URL` | `UrlRestrictions{}` — no scheme or host allow-list |
| `golem.Path` | `PathSpec{InOut, Any}` — direction and kind are lost |
| every integer | `NumericRestrictions` = none — no min/max |

This is wire-compatible: restrictions are validated server-side and no graph
travels at RPC time. But an author who means "an *input* *directory*" gets an
unconstrained path, and a reader (or a model) is told less than the author knew.

Fixing it needs a way to attach restrictions to a Go type — most likely the
`Quantity[U]` pattern, a marker type parameter carrying the spec. Until then the
gap should be asserted by a test so it stays a decision rather than an accident.

## 4. `QuotaToken` and `PermissionCard` have no Go representation

Nothing in `codec_scalar.go` produces `QuotaTokenType` or `PermissionCardType`,
although the WIT has both and `validate_host_managed_agent_bridge_policy` in the
CLI explicitly *allows* them in guest RPC method inputs and outputs — Rust tests
exactly that case. A Go agent therefore cannot take or return one.

Related: `rpc.go` stubs the permission scope card as `None` via `noScopeCard()`,
so agent permission scoping (main #3712) is not expressible either.

## 5. `containsStream` is approximate for recursive types

`codec.containsStream` propagates a nested stream to every enclosing type, which
is what makes `Trigger`/`Schedule` refuse a stream-bearing method. A cycle can
hide one: if `A → B → A` and the stream is found after `B` has already closed,
`B` is not marked.

The direction is conservative — it permits a call that should have been refused,
rather than refusing a legal one — and a stream inside a recursive type is not
expressible today. The exact answer is a graph walk
(`core/schema.Ref.ContainsStream`), which the dynamic paths already use.

## 6. There is no cross-language canonical-JSON fixture corpus

GOL-653 exists because MoonBit and Scala each diverged from the host's canonical
form, and nothing caught it. Go's tests state the contract, but they are Go's
tests: they cannot stop another SDK drifting.

The durable fix, which GOL-653 itself asks for: a committed corpus emitted by
`golem-schema` — graph, value, expected canonical JSON, expected JSON Schema —
consumed by every SDK's test suite. `sdks/go/core/schema/testdata/canonical/` is
where Go would read it from.

## 7. Toolchain and tooling traps

- **`go version` disagrees with itself.** At the repo root it reports the host
  Go (1.26.5); inside `sdks/go/golem` the `toolchain` directive re-execs stock
  1.27.1 from the module cache. Stock 1.27.1 parses generic methods, so plain
  `go build` of SDK code works — but building a *component* needs the patched
  `golemcloud/go` fork. Anything that shells out to `go` must resolve the
  toolchain deliberately (`app/build/go_toolchain.rs`), never `Command::new("go")`,
  or it behaves differently in CI and locally.
- **`gofmt` on `PATH` is the host's.** It cannot parse generic methods and
  reports spurious errors. Use `$(go env GOROOT)/bin/gofmt`.
- **`go build ./...` on a component fails natively** with
  `invalid reference to runtime.sbrk`. That is expected — a component is a
  wasip1 artifact — but the error says nothing useful. Use
  `GOOS=wasip1 GOARCH=wasm go vet ./...` to type-check.
- **`golem-temp/extracted-component-metadata/*.json` can be stale.** A build
  that rebuilt the wasm did not refresh it, and reading it gave a confidently
  wrong answer about which agents a component publishes. Check the binary.

## 8. `test-components/agent-sdk-go/module/go.mod` hardcodes an absolute path

The `replace` points at `/home/noise64/workspace/golem-alt-02/sdks/go/golem`
despite a comment in the same file saying the path is relative. CI survives only
because `build-components.sh` exports `GOLEM_GO_PATH` and the CLI rewrites the
file. It should be relative, and will have to be touched anyway when the second
module (`core`) is added to the require set.

## 9. The demo app cannot demonstrate `DeclareRemoteAgent`

Declaring an agent the same component implements is (correctly) a definition
error, so showing a remote call needs a second component. The playground demo is
a single component today. This is the same shape as the guest-bridge work, so
the two should be done together.
