# Golem SDK for MoonBit — Examples

Example agents built with the Golem SDK for MoonBit, targeting **Golem 1.4**.

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
moon run cmd -- agents ../golem_sdk_example1
```

## Building

Requires [golem-cli](https://github.com/golemcloud/golem/releases), `wasm-tools`, and the MoonBit toolchain.

```sh
# Build for local (debug):
golem build -L

# Build for release:
golem build -L -P release
```

## Deploying

```sh
# Deploy to a local Golem server:
golem deploy -L -Y

# Deploy with reset (clears existing agent state):
golem deploy -L --reset -Y
```
