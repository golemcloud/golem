---
name: golem-undo-agent-state
description: "Reverting agent state by undoing operations. Use when reverting invocations, rolling back agent state, or recovering from errors by undoing recorded operations."
---

# Undoing Agent State

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

The `<AGENT_ID>` format depends on the agent's language — refer to the language-specific invocation skills for the exact syntax.

## `agent revert` — Undo Operations

Reverts an agent by undoing its last recorded operations. Useful for recovering from errors or unwanted state changes.

```shell
golem agent revert <AGENT_ID> [OPTIONS]
```

### Flags

| Flag | Description |
|------|-------------|
| `--last-oplog-index <INDEX>` | Revert to a specific oplog index. Cannot be combined with `--number-of-invocations`. |
| `--number-of-invocations <N>` | Revert the last N invocations. Cannot be combined with `--last-oplog-index`. |

### Examples

Revert the last invocation:
```shell
golem agent revert <AGENT_ID> --number-of-invocations 1
```

Revert to a specific oplog index (use `agent oplog` to find the index):
```shell
golem agent revert <AGENT_ID> --last-oplog-index 42
```
