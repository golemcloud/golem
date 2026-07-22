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

## `agent file-contents` — Download a File from an Agent

```shell
golem agent file-contents <AGENT_ID> <PATH> [--output <LOCAL_FILE>]
```

Downloads one file from the agent's virtual filesystem and saves it to the host filesystem.

### Arguments

| Argument | Description |
|----------|-------------|
| `<AGENT_ID>` | The agent to inspect, e.g. `CounterAgent("c1")` |
| `<PATH>` | Absolute file path inside the agent filesystem, e.g. `/data/log.txt` |
| `--output <LOCAL_FILE>` | Local host path to write. If omitted, the file is saved in the current directory using the guest file basename, or `output.bin` if no basename is available. |

### Examples

Save `/data/log.txt` as `./log.txt` in the current directory:
```shell
golem agent file-contents 'CounterAgent("c1")' /data/log.txt
```

Save to an explicit local path:
```shell
golem agent file-contents 'CounterAgent("c1")' /data/log.txt --output ./downloads/counter.log
```

Get machine-readable metadata about the saved file:
```shell
golem --format json agent file-contents 'CounterAgent("c1")' /data/log.txt --output ./downloads/counter.log
```

Structured output uses `$type: "agent.file-contents"` and reports fields such as `saved`, `outputPath`, and `bytes`. The file bytes are written to the host file, not embedded in the structured output document.
