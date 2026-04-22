---
name: golem-view-agent-files
description: "Listing files in an agent's virtual filesystem. Use when browsing or inspecting files stored by a running agent instance."
---

# Viewing Agent Files

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

The `<AGENT_ID>` format depends on the agent's language — refer to the language-specific invocation skills for the exact syntax.

## `agent files` — List Files in an Agent's Directory

```shell
golem agent files <AGENT_ID> [PATH]
```

Lists the files and directories in the given agent's virtual filesystem. If `[PATH]` is omitted it defaults to `/` (the root directory).

### Arguments

| Argument | Description |
|----------|-------------|
| `<AGENT_ID>` | The agent to inspect, e.g. `CounterAgent("c1")` |
| `[PATH]` | Directory path to list (default: `/`) |

### Examples

List everything under the root directory:
```shell
golem agent files 'CounterAgent("c1")'
```

List files in a specific subdirectory:
```shell
golem agent files 'CounterAgent("c1")' /data
```

Get output as JSON:
```shell
golem agent files 'CounterAgent("c1")' --format json
```
