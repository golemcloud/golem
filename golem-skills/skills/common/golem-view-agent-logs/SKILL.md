---
name: golem-view-agent-logs
description: "Viewing agent logs and output. Use when streaming agent stdout/stderr/log channels or understanding how to observe agent output at runtime."
---

# Viewing Agent Logs

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## `agent invoke` — Log Streaming During Invocation

By default, `agent invoke` streams the agent's stdout, stderr, and log channels live while the invocation runs:

```shell
golem agent invoke <AGENT_TYPE_NAME> <AGENT_NAME> <FUNCTION_NAME> [ARGUMENTS...]
```

### Streaming flags

| Flag | Description |
|------|-------------|
| `--no-stream` / `-n` | Disable live streaming entirely — only print the invocation result |
| `--stream-no-log-level` | Hide log levels (e.g. `INFO`, `ERROR`) from streamed output |
| `--stream-no-timestamp` | Hide timestamps from streamed output |
| `--logs-only` | Only show entries coming from the agent — suppress invocation markers and stream status messages |

These flags can be appended to any `agent invoke` command. For the invocation syntax itself (agent ID, function name, arguments), refer to the language-specific invocation skills.

### Examples

Disable streaming and only get the result:
```shell
golem agent invoke ... --no-stream
```

Clean log output without timestamps or levels:
```shell
golem agent invoke ... --stream-no-timestamp --stream-no-log-level
```

Show only agent-emitted output:
```shell
golem agent invoke ... --logs-only
```

## `agent stream` — Live Stream an Agent's Output

Connect to a running agent and live stream its stdout, stderr, and log channels via WebSocket. The stream reconnects automatically if the connection drops.

```shell
golem agent stream <AGENT_TYPE_NAME> <AGENT_NAME>
```

This is useful for observing an agent that is already running or will be invoked separately.

### Flags

| Flag | Description |
|------|-------------|
| `--stream-no-log-level` | Hide log levels from output |
| `--stream-no-timestamp` | Hide timestamps from output |
| `--logs-only` | Only show agent-emitted entries, suppress invocation markers and stream status |

The `<AGENT_ID>` format depends on the agent's language — refer to the language-specific invocation skills for the exact syntax.

### Examples

Stream all output from an agent:
```shell
golem agent stream <AGENT_ID>
```

Stream only agent logs without decorations:
```shell
golem agent stream <AGENT_ID> --stream-no-timestamp --stream-no-log-level --logs-only
```


