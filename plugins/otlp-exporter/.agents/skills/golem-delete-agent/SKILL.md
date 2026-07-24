---
name: golem-delete-agent
description: "Deleting an agent instance. Use when removing, deleting, or destroying a running or idle agent instance via the CLI."
---

# Deleting an Agent

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

The `<AGENT_ID>` format depends on the agent's language — refer to the language-specific invocation skills for the exact syntax.

## `agent delete` — Delete an Agent Instance

Permanently removes an agent instance and all its associated state (oplog, memory, etc.).

```shell
golem agent delete <AGENT_ID>
```

### Examples

Delete a specific agent instance:
```shell
golem agent delete CounterAgent("my-counter")
```

Delete an agent in a specific environment:
```shell
golem agent delete my-env/CounterAgent("my-counter")
```
