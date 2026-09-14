# Golem SDK for MoonBit — Examples

Example agents and tool middleware built with MoonBit Golem SDK 0.5.x, targeting **Golem 1.6**.

## Agents

- **Counter** — Simple stateful counter with increment, decrement, and get-value methods
- **TaskManager** — Task management with custom data types (`Priority` enum, `TaskInfo` struct)
- **VisionAgent** — Multimodal agent accepting text or image input
- **RpcExampleAgent** — Demonstrates agent-to-agent RPC using generated client stubs

## Agent tools

`golem_moonbit_examples/canonical_tools.mbt` contains the canonical `grep` and `git` agent-tool
definitions shared with the Rust and Scala SDK parity suites. It demonstrates root commands, pure
dispatchers, nested subtrees, inherited globals, positionals/options/flags, refinements,
constraints, result formatters, typed custom errors, stdin/stdout injection, and full `UInt64`
bounds.

The `agents` build step generates:

- `golem_tools.mbt` — descriptors, registration, custom error schemas, and dispatchers.
- `golem_tool_clients.mbt` — typed clients for root commands and nested subcommand trees.

Both files are generated and must not be edited manually. To regenerate them directly while
iterating on this repository:

```sh
cd ../golem_sdk_tools
moon run cmd -- agents ../golem_sdk_example1 \
  --component-dir golem_moonbit_examples \
  --role ordinary
```

## Tool middleware

`golem_tool_middleware_examples/middleware.mbt` is a separate **pure middleware component**. It
contains:

- `MessagePolicy`, a monomorphic policy that forwards allowed messages once and short-circuits
  blocked messages without calling the underlying tool;
- `audit`, a universal pass-through that inspects runtime metadata and forwards opaque carriers;
- `FileAdapter`, a compact monomorphic adapter that presents `PublicFiles` while expecting
  `Storage` from the next inner layer.

The `moonbit-tool-middleware-local` template runs both generators with role `tool-middleware`,
embeds SDK world `tool-middleware-guest`, and produces
`golem_tool_middleware_examples.tool-middleware.wasm`. The package imports `tool-core` and
`tool-middleware`, but not ambient `tool`/`golem:tool/host`.

Generated `golem_reexports.mbt`, `golem_tool_middlewares.mbt`, `.golem-sdk-role`, and the generated
entries in `moon.pkg` must not be edited manually. To regenerate this component directly:

```sh
cd ../golem_sdk_tools
moon run cmd -- reexports ../golem_sdk \
  ../golem_sdk_example1/golem_tool_middleware_examples \
  --role tool-middleware
moon run cmd -- agents ../golem_sdk_example1 \
  --component-dir golem_tool_middleware_examples \
  --role tool-middleware
```

See the SDK and generator READMEs for the exact monomorphic/universal signatures, capability and
stream lifetime rules, same-package tool-shape limitation, and runtime ownership of placement and
chain ordering.

## Building

Requires [golem-cli](https://github.com/golemcloud/golem/releases), `wasm-tools`, and the MoonBit toolchain.

```sh
# Build both the ordinary and pure middleware components for local (debug):
golem build -L

# Build both components for release:
golem build -L -P release
```

## Deploying

```sh
# Deploy to a local Golem server:
golem deploy -L -Y

# Deploy with reset (clears existing agent state):
golem deploy -L --reset -Y
```
