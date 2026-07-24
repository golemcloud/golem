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
| `--stream-no-log-level` | Hide log levels (e.g. `INFO`, `ERROR`) from text streamed output. Structured formats still include `level`. |
| `--stream-no-timestamp` | Hide timestamps from text streamed output. Structured formats still include `timestamp`. |
| `--logs-only` | Only show entries coming from the agent — suppress invocation markers and stream status messages/events. |

These flags can be appended to any `agent invoke` command. For the invocation syntax itself (agent ID, function name, arguments), refer to the language-specific invocation skills.

### Structured Streaming Output

Streaming commands emit multiple structured output documents, one document per event. For JSON and YAML, parse stdout as a sequence of documents, not as one array or object.

When `golem` or `golem-cli` is run with `--format toon`, structured stdout is emitted as a sequence of framed TOON documents:

```text
@toon
<one TOON document>
@end
```

Parse stdout by splitting on exact `@toon` and `@end` marker lines. The content between them is one TOON document. In non-text formats, stderr may contain progress or diagnostic text and should not be parsed as the structured payload.

Stream events use `$type: "agent.stream"` and a rich event shape with common fields such as `timestamp`, `kind`, `level`, `context`, and `message`. Agent-facing code should branch on `kind` rather than parsing the message text. Typical `kind` values include `log`, `stdout`, `stderr`, `stream-closed`, `stream-error`, `invocation-started`, `invocation-finished`, and `missed-messages`.

`--stream-no-log-level` and `--stream-no-timestamp` affect only text output. In structured formats (`json`, `yaml`, `toon`), `level` and `timestamp` fields are always present for reliable parsing.

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
| `--stream-no-log-level` | Hide log levels from text output. Structured formats still include `level`. |
| `--stream-no-timestamp` | Hide timestamps from text output. Structured formats still include `timestamp`. |
| `--logs-only` | Only show agent-emitted entries, suppress invocation markers and stream status messages/events. |

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
