# Golem SDK for MoonBit — Examples

Example agents built with the Golem SDK for MoonBit, targeting **Golem 1.4**.

## Agents

- **Counter** — Simple stateful counter with increment, decrement, and get-value methods
- **TaskManager** — Task management with custom data types (`Priority` enum, `TaskInfo` struct)
- **VisionAgent** — Multimodal agent accepting text or image input
- **RpcExampleAgent** — Demonstrates agent-to-agent RPC using generated client stubs

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
