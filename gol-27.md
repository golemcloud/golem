# GOL-27 — Rust tool-middleware definition macros

## References and status

- Linear: [GOL-27](https://linear.app/golem-cloud/issue/GOL-27/rust-tool-middleware-definition-macros-monomorphic-and-universal)
- Product specification: [Agent Tools](https://app.notion.com/p/Agent-Tools-356a4cb355ec80c8b86efee765edf472), especially sections 6.2 and 6.3
- Current status: In Progress, Cycle 3
- Recommended estimate: 8 points, increased from 5 because the implementation requires generated typed underlying proxies, owned affine-value forwarding, multiple guest worlds, and cross-crate macro validation

## Implementation progress

Updated continuously while executing this plan on 2026-08-24.

| Phase | Scope | Status |
| --- | --- | --- |
| 1 | WIT and owned values | Complete — Oracle reviewed |
| 2 | Backend-neutral generation | Complete — Oracle reviewed |
| 3 | Generated middleware surface | Complete — Oracle reviewed |
| 4 | Authoring macros | Not started |
| 5 | Guest boundary | Not started |
| 6 | Acceptance coverage | Not started |

Progress log:

- Started phase 1 by reviewing the approved plan, current worktree, repository WIT/SDK guidance, and the GOL-27/GOL-39/GOL-439 ownership boundaries.
- Phase 1 implementation added the canonical middleware descriptor/scope/resource ABI, the middleware guest interface, pure and combined worlds, synchronized WIT copies, and explicit consuming typed-value codecs in `golem-schema` and `golem-rust`.
- The canonical pure world is named `tool-middleware` rather than `tool-middleware-guest`: WIT resolves package-level interface/world references by name, so giving both the world and exported interface the same name makes `export tool-middleware-guest` resolve to the world and fails parsing. The exported ABI remains `golem:tool/tool-middleware-guest@0.1.0` as required.
- Phase 1 verification passed: WIT synchronization, `golem-rust` build, 299 default `golem-schema` library tests, and 4 dedicated guest-feature tests for nested secret/quota forwarding, duplicate-handle rejection, second-transfer rejection, and malformed aliased wire nodes. The dedicated target is included in `cargo make unit-tests` so CI executes it. `cargo make check-wit` will become clean once the synchronized copies are committed, because that task intentionally reports any uncommitted generated WIT delta.
- Phase 1 Oracle review found the initial guest-feature integration target was not part of normal CI. The review finding was fixed by wiring it into `unit-tests`; the Oracle follow-up confirmed there are no remaining phase 1 blockers and that the cargo-make task follows repository conventions.
- Phase 1 was committed as `04094f061`; `cargo make check-wit` then passed with no synchronized-copy drift.
- Started phase 2 by mapping the existing generated ambient client, leaf invoker, canonical input, subtree projection, custom-error, and stream result paths that must be shared with an underlying-resource backend.
- Phase 2 now routes generated ambient clients through the SDK-owned `AmbientToolRpc` backend instead of an export-world `ToolRpc`. Existing callers that pass the agentic-world resource to public client helpers remain source-compatible.
- Leaf dispatch is split into shared owned decode, borrowed-instance dispatch, and decoded call stages. The static guest entry preserves its prior zero-sized implementation contract and validation order, while middleware generation can reuse the instance path without a dangling reference. Canonical record and subtree forwarding now move decoded fields and use the owned typed encoder rather than cloning the complete value tree.
- Phase 2 verification passed: both SDK crates build, all 98 macro tests pass, all 157 existing-plus-instance `tool` tests pass, all 9 canonical tool tests pass, and Rust formatting is clean.
- Phase 2 Oracle review found no correctness, compatibility, ownership, or phase-boundary blockers and recommended landing the phase.
- Phase 2 was committed as `330bed2db`; phase 3 started with the shared runtime error/resource model and generated typed middleware projections.
- Phase 3 now has the SDK-owned middleware metadata, invocation result, exact six-arm `ToolInvokeError<E>`, and non-`Clone` mutable `UnderlyingTool` wrapper. The wrapper owns input/stdin/result/stdout transfer, decodes only custom errors, and preserves all five protocol errors exactly.
- `#[tool_definition]` now emits visibility-matched `<Trait>Underlying` and `<Trait>Middleware<U = ...>` surfaces, including recursive flattened subtree leaves, inherited-global/alias omission, caller-side stream projection, native descriptor factories, and direct-versus-flattened collision diagnostics. Auto-injected `Principal` remains visible to a typed middleware method when the authored tool trait requests it, while generated underlying methods omit it so middleware cannot replace the runtime-bound principal.
- The shared authoring layer exposes SDK-owned native tool metadata and common principal/tool resource bindings independently of a middleware export world. The existing complete tool test target compiles with all generated middleware surfaces enabled.
- Phase 3 cross-crate validation passed with a public nested-subtree definition crate consumed by a separate middleware crate. The consumer implemented the generated flattened `ParentMiddleware` trait and invoked the generated `ParentUnderlying` proxy without a leaf implementation or registry entry in the definition crate.
- Phase 3 verification passed: both SDK crates build, all 101 macro tests pass, all 158 tool tests pass, all 9 canonical tool tests pass, the 6 focused middleware runtime tests pass, Rust formatting is clean, and targeted `-D warnings` Clippy checks pass for the macro crate, SDK library, and tool test. The broader all-target Clippy command reaches an unrelated pre-existing `unused_must_use` failure in `tests/agent.rs:507`.
- The initial phase 3 Oracle review found two blockers: `golem_rust::tool::Principal` was not recognized as the auto-injected principal path, and generated middleware/proxy bindings could collide with legal authored parameter names. The SDK principal re-export now uses the active guest world's nominal principal type, all three principal recognizers accept the shared path, and every generated middleware/proxy binding is freshened against the complete projected parameter set.
- The first Oracle follow-up confirmed the principal fix and the ordinary-identifier collision fix, then found that raw identifiers such as `r#underlying` still bypassed the freshness comparison. Freshening now normalizes projected identifiers with `IdentExt::unraw()`, and compile-shape coverage exercises both ordinary and raw forms of every generated binding's preferred name. The focused macro and tool checks pass after the correction. The final Oracle follow-up confirmed that the raw-identifier hole is closed and that phase 3 has no remaining blocker.
- Phase 3 bug-finder review found the same raw-identifier normalization requirement in direct-versus-flattened method collision diagnostics. Flattened path construction and direct-name comparison now normalize raw identifiers, while direct raw command identifiers are preserved in generated method signatures. The retained regression test passes, and the bug-finder follow-up found no new defects.

## Outcome

Add complete Rust SDK authoring support for both forms of tool middleware:

1. **Monomorphic middleware** wraps one statically known tool shape and presents the same or a different statically known shape.
2. **Universal middleware** transparently wraps any tool and operates on runtime tool metadata and owned raw `TypedSchemaValue` values.

The SDK will export middleware metadata and dispatch middleware invocations received through the new `golem:tool/tool-middleware-guest@0.1.0` interface. Middleware reaches only the next inner layer through a runtime-created `underlying-tool` resource.

This ticket ends at the component boundary. It does not persist middleware bindings or execute a complete middleware chain in the worker executor.

## Product contract

The implementation must preserve these invariants from the Agent Tools specification:

- Monomorphic and universal middleware have distinct Rust authoring entry points.
- A monomorphic descriptor has `scope: monomorphic` with:
  - `presented`: the tool shape callers see;
  - `expected: Some(tool)`: the shape required of the next inner layer.
- A universal descriptor has `scope: universal`; it does not declare one presented shape and must preserve the wrapped tool's shape.
- Middleware owns control flow. It can call the underlying layer zero, one, or multiple times.
- `UnderlyingTool` is minted by the runtime for one outer invocation, has no public constructor, is non-`Clone`, and can invoke only its bound next layer.
- Calls through one underlying handle may be sequential but not overlapping.
- The runtime threads the original principal through the chain; middleware cannot replace it when invoking the next layer.
- Passing stdin to the underlying layer transfers ownership of that stream. Returned stdout is likewise owned and may be forwarded or consumed.
- Pure middleware components must not import `golem:tool/host`; next-layer dispatch is possible only through `UnderlyingTool`.
- All six `tool-error` cases must be preserved exactly. Only `custom-error` is decoded to or mapped between a tool's Rust error types.

The specification's older WIT examples use `value-tree`, Agent 1.5 principal types, and P2 stream names. GOL-27 must use the repository's current contracts instead:

- `golem:core/types@2.0.0.typed-schema-value`;
- `golem:agent/common@2.0.0.principal`;
- P3 `stream<u8>` ownership;
- asynchronous guest and underlying invocation.

## Scope

### Included

- Middleware metadata and resource types in canonical `golem:tool` WIT.
- Pure-middleware and combined tool/middleware guest worlds.
- Owned `TypedSchemaValue` conversion paths capable of carrying affine secret and quota handles.
- A public SDK wrapper for the runtime-owned `UnderlyingTool` resource.
- Exact middleware invocation error representation.
- Typed underlying proxies and middleware-facing traits generated by `#[tool_definition]`.
- `#[tool_middleware]` for monomorphic middleware.
- `#[universal_tool_middleware]` for universal middleware.
- Middleware descriptor and invoker registration.
- Implementations of middleware discovery, lookup, and invocation guest exports.
- Compile-time diagnostics, cross-crate support, renamed-SDK support, stream tests, affine-value tests, and WIT import inspection.

### Excluded

- Manifest syntax, persistence, ordering, logical chain composition, adjacent descriptor compatibility validation, and production of a resolved chain plan; those belong to GOL-39.
- Consumption of the resolved chain plan during execution, chain traversal, middleware guest invocation, next-layer handle minting and binding, principal propagation, universal-transparency validation, and final runtime result validation; those belong to GOL-439.
- An agent-facing or middleware-facing host API for discovering or invoking middleware.
- Changes to non-Rust authoring APIs beyond synchronizing canonical WIT copies.

GOL-27 publishes the descriptors and component ABI consumed by GOL-39 and GOL-439. It performs local typed command/input decoding and result-slot validation only; it does not decide whether a deployed chain is compatible or whether a universal middleware preserved the runtime tool shape.

### Downstream constraint

GOL-438 was canceled because its proposed agent-facing `tool-middleware-host` lookup conflicts with the no-host-access contract. If host-side or operator tooling needs middleware lookup, it must use an internal control-plane/runtime API rather than a component-facing WIT interface.

## ABI and ownership handoff

`invoke-tool-middleware` transfers the following owned values from GOL-439's runtime traversal into the guest:

| Argument | Guest ownership and meaning |
| --- | --- |
| middleware name | Owned identifier selecting one registry entry; unknown names do not construct middleware state. |
| tool name | Owned source-side tool identifier for this invocation. |
| tool metadata | Owned metadata for the next inner layer; it is the schema against which this layer's input is interpreted. |
| command path | Owned path selecting the presented command leaf. |
| input | Owned `TypedSchemaValue`, including any affine handles. |
| stdin | Optional owned P3 reader; forwarding transfers it to the next layer. |
| principal | Original authenticated principal, visible to middleware but not replaceable on inner invocation. |
| wrapped | Owned, non-`Clone` `UnderlyingTool` bound to this outer invocation, principal, and exactly one next layer. |

`UnderlyingTool::invoke` accepts an owned command path, input, and optional stdin. It does not accept tool name or principal: both are captured when GOL-439 mints the handle. It returns an owned invocation result and optional readable stdout. The handle is dropped when the outer guest call returns and cannot be retained across invocations.

Boundary validation is split deliberately:

- GOL-27 validates local typed projection, command/input decoding, custom-error codecs, and value/stdout slot shape.
- GOL-39 validates descriptor compatibility while producing the resolved chain plan.
- GOL-439 validates runtime universal transparency and the final result against the published tool shape.

## Public Rust API

### Error envelope

Introduce a middleware-specific error type instead of forcing runtime invocation failures into a tool's custom error enum:

```rust
pub enum ToolInvokeError<E> {
    InvalidToolName(String),
    InvalidCommandPath(Vec<String>),
    InvalidInput(String),
    ConstraintViolation(String),
    InvalidResult(String),
    Tool(E),
}
```

Required behavior:

- Decode an expected tool's custom error through its generated `ToolErrorSchema::from_error_payload_value` implementation.
- Encode a presented tool's custom error through its generated `ToolErrorSchema::to_error_payload_value` implementation. Adapter middleware may therefore use different expected and presented error types.
- Implement `From<E>` so middleware-created custom errors work naturally with `?`.
- Provide `map_tool`, which transforms only `Tool(E)` and leaves the five protocol variants unchanged.
- Map malformed guest input or input decode failures to `InvalidInput`; map malformed underlying results, custom-error payloads, presented results, or result encoding failures to `InvalidResult`.
- Preserve a pre-existing non-custom protocol variant exactly instead of replacing it with a newly generated decode/encode error.
- Do not reuse the current typed-client `ToolError<E>`. That type models ambient RPC failures versus remote tool failures, whereas `ToolInvokeError<E>` models the exact error channel of `underlying-tool.invoke` and `tool-middleware-guest.invoke-tool-middleware`.

This envelope is required because an ordinary tool trait such as `FileTool` returns only `FileError`, while `underlying-tool.invoke` may return any WIT `tool-error`. The exact canonical form

```rust
impl<F: FileTool> FileTool for PathPolicy<F>
```

cannot represent `invalid-input`, `constraint-violation`, or `invalid-result` without losing their identity. It is therefore not the emitted middleware contract. Generic or blanket middleware impls are rejected in this ticket; the generated typed underlying proxy is concrete and runtime-backed.

### Generated monomorphic API

For a definition named `FileTool`, `#[tool_definition]` additionally generates:

- `FileToolUnderlying`: a typed proxy over an SDK `UnderlyingTool`;
- `FileToolMiddleware<U = FileToolUnderlying>`: a middleware-facing trait with caller-side command signatures;
- hidden descriptor factories/marker implementations that let another crate recover the expected and presented tool descriptors without requiring a concrete tool implementation.

Transparent middleware uses the default underlying type:

```rust
pub struct PathPolicy {
    allowed_root: String,
}

impl PathPolicy {
    fn new() -> Self {
        Self {
            allowed_root: "/workspace/".to_string(),
        }
    }
}

#[tool_middleware(
    name = "path-policy",
    constructor = PathPolicy::new
)]
impl FileToolMiddleware for PathPolicy {
    async fn read(
        &self,
        underlying: &mut FileToolUnderlying,
        path: String,
    ) -> Result<Vec<u8>, ToolInvokeError<FileError>> {
        if !path.starts_with(&self.allowed_root) {
            return Err(FileError::OutsideAllowedRoot(path).into());
        }

        underlying.read(path).await
    }
}
```

An adapter selects a different generated underlying type:

```rust
#[tool_middleware(
    name = "grep-via-ripgrep",
    constructor = GrepAdapter::new
)]
impl GrepToolMiddleware<RipgrepToolUnderlying> for GrepAdapter {
    async fn grep(
        &self,
        underlying: &mut RipgrepToolUnderlying,
        pattern: String,
        files: Vec<String>,
    ) -> Result<Vec<GrepMatch>, ToolInvokeError<GrepError>> {
        underlying
            .search(pattern, files, RipgrepOptions::default())
            .await
            .map(convert_matches)
            .map_err(|error| error.map_tool(convert_ripgrep_error))
    }
}
```

The constructor is required, synchronous, infallible, takes no runtime handle, and is called at most once per outer `invoke-tool-middleware` call. An unknown middleware name returns before construction. For a known monomorphic middleware, guest dispatch constructs exactly one instance before command-path/input decoding and reuses it for the complete call, including every sequential underlying invocation. The underlying handle remains a method argument and is never stored in middleware state.

The initial attribute surface is deliberately small:

- `name = "..."` is required and validated as a tool identifier;
- `constructor = path` is required for monomorphic middleware;
- Rust doc comments on the impl provide descriptor documentation;
- aliases are emitted as an empty list until an alias-authoring requirement is added to the product contract.

### Generated stream projection

Typed underlying methods use the existing tool-client caller projection rather than the leaf implementation signature:

- stdin is an owned `InputStream` argument;
- a leaf `OutputStream` implementation parameter is omitted;
- stdout is returned to the middleware as an owned `InputStream`;
- value plus stdout returns `(T, InputStream)`;
- stdout-only returns `InputStream`;
- missing or unexpected value/stdout slots produce `ToolInvokeError::InvalidResult`.

A middleware that passes stdin to one underlying call cannot pass the same stream to a retry. This follows P3 ownership and must be visible in the generated Rust signature.

### Universal API

Universal middleware is a separate function macro and receives owned raw values:

```rust
#[universal_tool_middleware(name = "audit")]
async fn audit(
    tool_name: String,
    tool_metadata: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    principal: Principal,
    mut underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<TypedSchemaValue>> {
    record_invocation(&tool_name, &command_path, &principal);
    underlying.invoke(command_path, input, stdin).await
}
```

The exact parameter order is fixed by the macro and mirrors the WIT guest function. The custom-error payload remains a raw `TypedSchemaValue`; the universal form cannot assume a statically known Rust error type. Input and output are forwarded semantically unchanged after ownership-safe decoding and encoding unless the body intentionally transforms them. The SDK does not promise byte-identical schema-graph serialization. GOL-439 performs the authoritative transparency check against the runtime tool metadata.

## Detailed implementation plan

### 1. Add the canonical middleware WIT contract

Change `wit/deps/golem-tool/common.wit`:

- Add `tool-middleware` with `name`, `aliases`, `doc`, and `scope`.
- Add `tool-middleware-scope` with `monomorphic(monomorphic-scope)` and `universal`.
- Add `monomorphic-scope` with `presented: tool` and `expected: option<tool>`.
- Add the constructor-less `underlying-tool` resource.
- Define `underlying-tool.invoke` as asynchronous and accept:
  - `command-path: list<string>`;
  - `input: typed-schema-value`;
  - `stdin: option<stream<u8>>`.
- Return `result<invocation-result, tool-error>` without an RPC wrapper.

Add `wit/deps/golem-tool/middleware.wit`:

- Define `tool-middleware-guest`.
- Add synchronous `discover-tool-middlewares` and `get-tool-middleware` functions.
- Add asynchronous `invoke-tool-middleware` accepting:
  - middleware name;
  - source-side tool name;
  - next-layer tool metadata;
  - command path;
  - owned typed input;
  - optional owned stdin;
  - Agent 2.0 principal;
  - owned `underlying-tool`.
- Return `result<invocation-result, tool-error>`.
- Define `tool-middleware-guest`, a pure world that explicitly imports `common`, `golem:api/host@1.5.0`, and `golem:agent/host@2.0.0`, exports the middleware guest interface, and never imports `golem:tool/host`.
- Define `tool-and-tool-middleware-guest`, a combined world that includes the existing `tool-guest` world and additionally exports the middleware guest interface. Its tool half retains the existing `golem:tool/host` import.

Update `sdks/rust/golem-rust/wit/golem-rust.wit`:

- Leave the existing `golem-agentic` world unchanged so ordinary rebuilt agentic components keep their current imports and exports.
- Add `golem-tool-middleware`, a pure Rust middleware export world used by middleware-only components. It must not include the current `golem-rust` world because that world imports `golem:tool/host`.
- Add `golem-agentic-tool-middleware`, an opt-in combined world that includes the existing `golem-agentic` world and additionally exports `tool-middleware-guest`.

Do not add the new guest export to the worker executor's `wit/host.wit` in this ticket. Executor-side typed invocation and chain traversal belong to GOL-439.

Run `cargo make wit` from the repository root and commit all synchronized `wit/deps` copies. Use `cargo make check-wit` to verify no drift remains.

### 2. Add owned affine typed-value conversion

Middleware differs from current metadata and client paths because it must receive and forward owned raw typed values that may contain host-managed resources.

The schema layer already has the hard ownership machinery: consuming `decode_value` lifts guest handles and rejects aliasing/unconsumed handles, while guest encoding preflights aliases and transfers take-once handle cells. Keep the GOL-27 delta limited to typed wrappers and generated field consumption.

Extend `golem-schema/src/schema/wit/decode.rs` and its public exports:

- Keep existing borrowed `decode_typed` behavior for pure-value boundaries that reject resource handles.
- Add `decode_typed_owned(wire::TypedSchemaValue)` by decoding the graph and delegating the owned value tree to the existing consuming `decode_value` traversal.
- Use owned typed decoding for middleware input, underlying results, and custom-error payloads.

Extend `golem-schema/src/schema/wit/encode.rs` and SDK facades in `sdks/rust/golem-rust/src/lib.rs`:

- Expose `encode_typed_owned(TypedSchemaValue)` for an explicit transfer API, delegating it to the existing guest encoder rather than adding another value walker.
- Preserve the existing alias preflight and take-once transfer behavior of guest secret and quota handle cells.
- Keep borrowed helpers for existing callers whose values are guaranteed to contain no affine resources.

Update generated canonical-input handling:

- Consume decoded record fields when constructing typed middleware arguments instead of cloning the complete value tree.
- Ensure nested record/list/variant/union paths cannot duplicate a capability or produce a duplicate occurrence at encoding. Existing `FromSchema` implementations may clone Rust wrappers that share one take-once cell; that is valid as long as the underlying capability remains unique.
- Return `InvalidInput` for malformed input before invoking middleware.
- Return `InvalidResult` if monomorphic middleware returns a value or stream slot inconsistent with its generated presented signature.

Add focused `golem-schema` tests for nested secret and quota handles, including successful one-time forwarding, duplicate-reference rejection, and second-transfer failure.

### 3. Add runtime middleware types and exact error mapping

Create a shared `golem_rust::tool` authoring/runtime layer that does not depend on a particular exported guest world:

- Move or re-export tool descriptors, canonical projection helpers, registries, errors, and stream aliases through this layer.
- Keep existing `golem_rust::agentic::*` tool re-exports source-compatible.
- Compile the shared layer whenever ordinary agentic, pure middleware, or combined middleware export support is enabled.
- Map `golem:agent/common@2.0.0` and `golem:tool/common@0.1.0` from every bindgen world to one shared binding/model module so pure and combined worlds do not leak nominally different public Rust types.
- Expose the universal signature through SDK-owned types: native `golem_schema::schema::tool::Tool` metadata, the shared `Principal` re-export, `TypedSchemaValue`, shared `InputStream`, SDK `InvocationResult`, `UnderlyingTool`, and `ToolInvokeError<TypedSchemaValue>`.
- Represent middleware descriptors as an SDK model and convert to each world's WIT type only in the thin guest-export adapter.

Generated ambient clients must name an SDK backend abstraction rather than a specific world's `ToolRpc` binding. They remain type-checkable in a definition crate used by pure middleware, but the ambient backend becomes link-reachable only when a client is actually constructed or called. The compiled pure-component import inspection is authoritative: an unused generated client must not introduce `golem:tool/host`.

Add middleware-owned runtime code under `sdks/rust/golem-rust/src/tool/`, keeping it separate from the ambient backend:

- `tool_middleware.rs` for public `UnderlyingTool`, `ToolInvokeError<E>`, raw invocation/result helpers, and wire conversions.
- Keep the raw WIT resource field private and expose only `pub(crate)` construction from guest dispatch.
- Do not implement `Clone` for `UnderlyingTool` or generated typed proxies.
- Make invocation require `&mut self`, preventing overlapping calls through safe Rust while allowing sequential retries.
- Make `UnderlyingTool::invoke` consume owned command input and stdin and return owned result/stdout.
- Implement exact conversion for every WIT `tool-error` arm.
- Implement custom-error encoding/decoding and `map_tool` without converting protocol variants.

Share result-slot validation with the current typed client where possible, but do not alter the current ambient `ToolError<E>` API or collapse middleware protocol errors into `RpcError::Protocol`.

### 4. Refactor tool-definition generation around invocation backends

The current generator in `sdks/rust/golem-rust-macro/src/tool/client.rs` directly constructs `ToolRpc`. Refactor only the reusable command projection pieces so both clients and underlying proxies use one source of truth:

- command and schema path construction;
- inherited-global and subtree handling;
- canonical input record construction;
- custom-error payload encoding/decoding;
- result and stdout shape projection;
- generated name-collision avoidance.

Keep backend-specific operations separate:

- `FooClient` constructs and invokes ambient `ToolRpc` and returns the existing `ToolError<E>`.
- `FooUnderlying` wraps a supplied `UnderlyingTool`, has no constructor, and returns `ToolInvokeError<E>`.

Similarly separate raw argument decoding from the final Rust call expression in `definition.rs`:

- preserve existing leaf-tool dispatch behavior;
- add an instance-based middleware dispatch path that invokes a constructed `&self` rather than the current zero-sized dangling-reference leaf implementation path;
- share decoding and encoding logic so middleware and leaf tools cannot diverge on command paths, inherited globals, constraints, streams, or custom errors.

Verify the refactor with existing tool tests before adding middleware behavior.

### 5. Generate middleware projections from `#[tool_definition]`

Extend `tool_definition_impl` to emit the middleware surface alongside the existing trait, descriptor, and typed client:

- Generate `<Trait>Underlying` with inherent async methods for every invokable command, including recursively flattened descendant leaves.
- Generate `<Trait>Middleware<U = <Trait>Underlying>` using caller-facing method signatures and `ToolInvokeError<CustomError>` returns.
- Give each middleware method an `underlying: &mut U` argument immediately after the receiver.
- Add a required hidden annotation item so implementations missing `#[tool_middleware]` fail with a targeted compiler error, matching the existing `#[tool_implementation]` enforcement pattern.
- Generate hidden public descriptor factories and marker traits for resolving presented and expected metadata across crate boundaries.
- Ensure a definition-only crate can publish these generated types without registering a leaf implementation.

The generated underlying type must retain enough descriptor information for adapter middleware to select a different expected shape. The monomorphic macro must use descriptor factories, not the process-global tool registry, because the expected tool may be defined in another crate and need not be registered as a leaf in the middleware component.

Grafted subtree definitions require a complete recursive projection rather than exposing the non-invokable subtree placeholder method:

- Recursively project every presented command body, including descendant leaves, into the middleware trait.
- Keep an original method name for a leaf declared directly on the definition. Name descendant methods by joining the Rust subtree-method path and leaf method with `__`, for example `remote__add` and `remote__origin__set_url`.
- Detect and reject collisions between flattened paths and directly authored method names with a targeted diagnostic; never silently rename one command.
- Give each projected method the effective inherited globals and leaf arguments exactly once, applying the same alias de-projection and omission rules as the existing canonical client.
- Do not generate a handler for a subtree dispatcher that has no command body.
- Generate the same recursive flattened methods on typed underlying proxies, rooted in the expected descriptor.
- Resolve raw command aliases to canonical command indices before routing to the corresponding projected method.

This representation lets one concrete middleware state intercept the complete presented tool without associated handler objects or lifetime-bearing subtree state. Add nested transparent and adapter fixtures to lock down method naming, inherited globals, alias routing, and expected/presented path differences.

### 6. Implement `#[tool_middleware]`

Add a new macro module under `sdks/rust/golem-rust-macro/src/tool/` and export the proc macro from `golem-rust-macro/src/lib.rs`.

Parse and validate:

- the attribute is on a trait impl;
- the implemented trait is a generated `<Presented>Middleware` trait;
- the impl is concrete, not generic or blanket;
- the optional middleware trait type argument is a generated underlying proxy;
- `name` is present and identifier-shaped; the registry detects duplicate names across independent macro expansions when component registration runs;
- `constructor` is present;
- Rust's type checker verifies that the constructor is synchronous, infallible, zero-argument, and returns the impl's `Self` type;
- only supported attribute keys are accepted;
- middleware methods and stream projections match their generated trait signatures.

Expansion must:

- inject the hidden middleware annotation item;
- build metadata with `presented` from the middleware trait descriptor;
- build `expected: Some(...)` from the selected underlying proxy descriptor;
- emit `scope: monomorphic`;
- create an invoker that constructs one state instance per outer call, wraps the runtime resource in the selected generated underlying proxy, and dispatches the selected command on that instance;
- register metadata and the invoker through a generated `ctor`, following `#[tool_implementation]` conventions.

Use `proc_macro_crate` consistently so expansion works when `golem-rust` is renamed in `Cargo.toml`. Generated descriptor references must also work when the tool definition and middleware implementation live in different crates.

### 7. Implement `#[universal_tool_middleware]`

Add a separate function attribute macro rather than overloading the monomorphic impl macro.

Validate that the target:

- is an async free function;
- is non-generic;
- has the exact SDK-owned parameter and return shape;
- uses a valid middleware name;
- does not request a constructor or a statically known expected/presented tool.

Expansion must:

- preserve the authored function;
- create `scope: universal` metadata;
- generate an invoker adapter from WIT types to owned SDK types;
- pass tool name, full next-layer metadata, command path, input, stdin, principal, and `UnderlyingTool` through owned conversions without duplicating affine capabilities;
- encode the returned raw result or exact error;
- register the descriptor and invoker through a `ctor`.

Universal dispatch must not run typed command decoding or use a statically generated tool descriptor. Its transparency is enforced at the boundary by preserving metadata and validating the returned invocation shape in the runtime; GOL-27 must at least avoid any SDK transformation that silently changes the raw schema/value pair.

### 8. Add the middleware registry and guest exports

Add `tool_middleware_registry.rs` beside `tool_registry.rs`:

- Maintain a `BTreeMap<String, ToolMiddleware>` and a parallel invoker map.
- Reject duplicate canonical middleware names deterministically.
- Keep middleware and tool namespaces separate.
- Provide registration, discovery, lookup, and invoker lookup helpers.
- Provide a test-only clear helper following the current registry pattern.

Define a function-pointer invoker type whose future owns every invocation value, including the raw WIT `UnderlyingTool` resource. Copy the function pointer out of the `RefCell` registry before awaiting so no registry borrow crosses an async suspension point.

Add `tool_middleware_impl.rs`:

- Implement `discover-tool-middlewares` using the descriptor registry.
- Implement `get-tool-middleware`, returning `InvalidToolName` for an unknown middleware name unless WIT gains a dedicated middleware-name error.
- Implement `invoke-tool-middleware`, look up the invoker, move all arguments into it, and await it.
- Keep tool guest dispatch unchanged.

Wire exports in `sdks/rust/golem-rust/src/lib.rs`, the shared tool layer, and thin world-specific guest adapters using three explicit modes:

1. Existing `export_golem_agentic` remains ABI-compatible and exports only agent and tool guests.
2. New `export_golem_tool_middleware` selects the pure `golem-tool-middleware` world and exports only middleware guest.
3. New `export_golem_agentic_tool_middleware` selects the opt-in combined world and exports agent, tool, and middleware guests. The Cargo feature includes the dependencies of `export_golem_agentic` plus shared middleware support.

Feature gating must be deterministic:

- When the combined feature is enabled, emit only the combined export macro; gate off the ordinary and pure export macros. This makes `--all-features` compile without duplicate component exports.
- If pure middleware and ordinary agentic export features are enabled together without the combined feature, emit a targeted `compile_error!` directing the author to the combined feature.
- Compile the shared registry, descriptors, public types, and macros in all three modes.
- Map pure and combined bindgen common types through the shared modules defined in section 3, then convert SDK models to world-specific WIT records only at export functions.
- Keep generated ambient clients type-checkable through the SDK backend abstraction. Their mere presence must not add a tool-host import to a middleware-only component.

The compiled component import/export set, not Rust module presence, is the acceptance boundary for world purity and ABI compatibility.

### 9. Tests

#### Macro parsing and synthesis

Add direct parser/token tests in `golem-rust-macro` for:

- valid transparent and adapter impls;
- valid universal functions;
- missing or unknown attributes;
- invalid names;
- non-trait monomorphic targets;
- non-function universal targets;
- non-async universal functions;
- generic/blanket middleware impl rejection;
- synthesis of the constructor type assertion used by compiled fixtures;
- missing middleware annotation diagnostics.

Do not add unit tests that invoke `cargo`, `rustc`, or another compiler subprocess. Cross-crate and compiled-component checks belong in the SDK/CLI integration-test path.

#### Registry and dispatch

Add Rust SDK tests covering:

- discover/get metadata for monomorphic and universal middleware;
- exact `presented` and `expected` descriptors across crates;
- duplicate registration behavior;
- transparent pass-through;
- short-circuiting without an underlying call;
- multiple sequential underlying calls;
- constructor execution exactly once per known outer invocation;
- no constructor for an unknown middleware, and one constructor for a known middleware even when command/input decoding later fails;
- adapter input/output conversion;
- adapter custom-error mapping;
- exact preservation of all five non-custom protocol errors;
- invalid command, input, result, and stream shapes;
- nested-subtree transparent and adapter dispatch, flattened method-name collisions, inherited globals, and aliases;
- universal access to tool metadata, command path, principal, and raw values;
- universal semantic forwarding without schema/value modification;
- stdin ownership and readable stdout forwarding;
- nested secret and quota values moving through monomorphic and universal middleware exactly once.

Use a fake SDK underlying backend for host-independent dispatch tests. The fake must record call count and received values and allow each WIT error variant to be injected.

#### Cross-crate and component integration

Add integration coverage that builds reviewable fixture crates for:

- a tool definition crate consumed by a separate middleware crate;
- transparent and adapter middleware;
- a renamed `golem-rust` dependency;
- a pure middleware component;
- a combined tool-and-middleware component.

Include compile-fail fixtures for async, fallible, argument-taking, and wrong-return-type constructors, plus missing middleware annotations and invalid generic impls. Assert the diagnostic's stable explanatory text rather than compiler-specific formatting.

Inspect compiled component contracts and assert:

- the pure component does not import `golem:tool/host` and exports middleware guest only;
- the combined component exports agent, tool, and middleware guests and imports tool host when its tool/client half requires it;
- a component using only existing `export_golem_agentic` retains its prior import/export ABI;
- enabling all SDK features selects one combined export rather than producing duplicate exports.

### 10. Verification sequence

Run targeted checks in this order so failures remain attributable:

```shell
# Repository root: WIT synchronization and drift
cargo make wit
cargo make check-wit

# Schema ownership changes
cargo test -p golem-schema --lib

# Rust SDK
cd sdks/rust
cargo build -p golem-rust-macro
cargo build -p golem-rust
cargo build -p golem-rust --features export_golem_tool_middleware
cargo build -p golem-rust --features export_golem_agentic_tool_middleware
cargo test -p golem-rust --features export_golem_agentic
cargo fmt --check
cargo clippy -p golem-rust-macro -p golem-rust --all-targets --all-features -- -D warnings
```

Run the targeted CLI/SDK integration test that builds the cross-crate and WASM fixtures, including the pure-component import inspection. Do not run `cargo make test`.

## Suggested implementation slices

1. **WIT and owned values:** add/sync the contract and land owned typed wrappers over the existing affine codec with focused tests.
2. **Backend-neutral generation:** refactor existing client/dispatcher synthesis without changing current tool behavior.
3. **Generated middleware surface:** add `ToolInvokeError`, `UnderlyingTool`, generated typed proxies, and middleware traits.
4. **Authoring macros:** add monomorphic and universal macros with metadata registration and diagnostics.
5. **Guest boundary:** add registry, pure/combined worlds, and discover/get/invoke exports.
6. **Acceptance coverage:** add behavior, cross-crate, stream, affine-resource, and WIT import tests.

Each slice should preserve existing tool-definition, tool-implementation, and typed-client tests before the next slice begins.

## Acceptance criteria

GOL-27 is complete when:

- A Rust definition crate generates usable typed middleware surfaces for another crate.
- Transparent monomorphic middleware can reject, forward, retry, and transform a tool invocation.
- Adapter middleware publishes one exact descriptor and requires another exact descriptor.
- Universal middleware receives and forwards owned raw typed values and runtime metadata.
- All WIT `tool-error` variants survive middleware unchanged except explicitly mapped custom payloads.
- Flat and grafted-subtree definitions expose complete, collision-checked typed middleware surfaces.
- P3 stdin/stdout ownership is represented correctly in generated Rust signatures and runtime behavior.
- Secret and quota capabilities cross middleware without duplication, leaking, or accidental consumption; shared take-once Rust wrappers remain allowed.
- Discovery and lookup return complete middleware metadata.
- Guest invocation skips construction for an unknown name and constructs one monomorphic state instance for every known outer call.
- Cross-crate and renamed-SDK fixtures compile.
- A compiled pure middleware component has no `golem:tool/host` import, while the opt-in combined world exports all three guest interfaces.
- Existing `export_golem_agentic` component ABI remains unchanged and `--all-features` emits one non-conflicting combined export.
- Existing Rust SDK tool authoring and typed clients remain source-compatible and their tests pass.
- Canonical and synchronized WIT trees are clean under `cargo make check-wit`.

## Risks and checkpoints

- **Owned affine values:** this is the highest-risk shared change. Validate schema-level ownership before generating middleware code on top of it.
- **Bindgen world identity:** pure and combined worlds may generate nominally distinct Rust types. Keep conversions at one boundary and expose SDK-owned types to macros.
- **Macro duplication:** avoid copying the current client and invoker generator wholesale; share command projection while keeping ambient RPC and underlying-resource error semantics separate.
- **Stream retries:** stdin cannot be replayed after ownership transfer. Generated APIs must not imply otherwise.
- **Spec drift:** use current Agent 2.0, typed-schema-value, async, and P3 contracts even where older specification snippets show legacy types.
- **Host API boundary:** do not reintroduce GOL-438's canceled component-facing middleware lookup; operator discovery remains an internal control-plane/runtime concern.
- **Runtime handoff:** keep the ABI ownership table synchronized with GOL-439 so executor-side handle minting preserves principal and next-layer binding and runtime transparency validation remains outside the SDK.
