# Golem native tool authoring

`golem-native-tool` is the shared authoring contract for tools compiled into a Golem host. It is
not a guest SDK: a native tool runs with the executor's worker context and is installed by both the
registry service and worker executor at startup.

There is currently no production native-tool inventory. The production startup hooks
`compiled_native_tools` in the registry service and executor return empty lists. The executable
example is `NativeDurableHelper` in `golem-worker-executor-test-utils/src/lib.rs`, and the macro
contract is covered by `tests/native_tool.rs`.

## Define and implement a tool

Use `#[tool_definition]` and `#[tool_implementation]` together. The implementation's `context`
argument is the concrete executor worker context; it is not part of the command's serialized input.
The generated `native_tool_invoker()` factory exposes both metadata and
`definition(id, implementation_version)`:

```rust
#[golem_native_tool::tool_definition(version = "1.0.0")]
trait NativeDurableHelper {
    async fn touch(&self, context: &mut TestWorkerCtx) -> golem_native_tool::HostResult<()>;
}

struct NativeDurableHelperImpl;

#[golem_native_tool::tool_implementation]
impl NativeDurableHelper for NativeDurableHelperImpl {
    async fn touch(&self, ctx: &mut TestWorkerCtx) -> golem_native_tool::HostResult<()> {
        wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(ctx).await?;
        Ok(())
    }
}

let invoker = NativeDurableHelperImpl.native_tool_invoker();
```

Call the same host helper implementations used by component imports, as above. This preserves their
durability, policy checks, and panic behavior. Do not bypass them by reaching directly into host
state.

`HostResult<T>` is the infrastructure-error channel and accepts `?` from host helpers. A declared
tool error remains nested: `HostResult<Result<T, MyToolError>>`. The macro maps the inner error to a
declared remote tool error, while failure of the outer `HostResult` becomes an executor failure.
Do not turn infrastructure errors or host panics into declared tool errors.

## Install at startup

Create one `NativeToolDefinition` from the generated invoker and use that same definition in both
startup inventories:

- The registry's `NativeToolDescriptor` provisions protected, immutable metadata with ambient
  availability.
- The executor's `NativeToolRegistration` pairs the exact definition with a
  `NativeToolAdapter(invoker)` handler.

The definition identity includes the host tool ID, implementation version, tool metadata version,
and metadata digest. Startup and dispatch reject duplicate or mismatched identities. Changing an
implementation or its metadata therefore requires a new immutable implementation/tool version and
coordinated registry and executor entries; never replace an existing exact version.

Native tools are ambient: consumers do not add a top-level `tools.<name>.release` declaration.
They may add an `agents.<agent>.tools.<name>` binding to narrow invocation parameters or readable
configuration. `configKeysReadable` is intersected across the system/environment default and agent
binding and enforced when the native implementation calls the normal config host helper. Native
execution does not imply unrestricted configuration access.

## Verify

From the repository root:

```text
cargo test -p golem-native-tool
```

The end-to-end native fixture also participates in the worker executor `tool_streaming` tests.
