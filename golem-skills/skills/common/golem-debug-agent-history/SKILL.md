---
name: golem-debug-agent-history
description: "Querying the operation log. Use when dumping or searching the oplog, or debugging agent behavior from its recorded history."
---

# Debugging Agent History

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

The `<AGENT_ID>` format depends on the agent's language — refer to the language-specific invocation skills for the exact syntax.

To check agent metadata and status, load the `golem-get-agent-metadata` skill. To revert agent state, load the `golem-undo-agent-state` skill.

## `agent oplog` — Query the Operation Log

Dump or search an agent's operation log (oplog). The oplog records every operation the agent has performed — invocations, side effects, persistence boundaries, etc.

```shell
golem agent oplog <AGENT_ID> [OPTIONS]
```

### Flags

| Flag | Description |
|------|-------------|
| `--from <INDEX>` | Start from a specific oplog entry index. Cannot be combined with `--query`. |
| `--query <LUCENE_QUERY>` | Search oplog entries using a Lucene query string. Cannot be combined with `--from`. |

If neither `--from` nor `--query` is provided, the entire oplog is returned.

### Output

Each entry is printed with its index (e.g. `#00042:`) followed by a labeled header and fields. The entry types rendered are:

| Entry | Description |
|-------|-------------|
| `CREATE` | Agent creation — shows timestamp, component revision, env vars, parent, initial plugins |
| `CALL` | Host function call — shows function name, input, and result |
| `INVOKE` | Agent method invocation started — shows method name, idempotency key, and input |
| `INVOKE COMPLETED` | Invocation finished — shows consumed fuel and result |
| `ENQUEUED INVOCATION` | Pending method invocation — shows method name and idempotency key |
| `ENQUEUED AGENT INITIALIZATION` | Pending agent initialization |
| `ENQUEUED SAVE SNAPSHOT` / `ENQUEUED LOAD SNAPSHOT` | Pending snapshot operations |
| `ENQUEUED MANUAL UPDATE` | Pending manual update — shows target revision |
| `SUSPEND` | Agent suspended |
| `ERROR` | Error with retry info — shows retry-from index and error details |
| `LOG` | Log entry — shows level and message |
| `GROW MEMORY` | Memory growth — shows size increase |
| `CREATE RESOURCE` / `DROP RESOURCE` | Resource lifecycle — shows resource id |
| `BEGIN ATOMIC REGION` / `END ATOMIC REGION` | Atomic operation boundaries |
| `BEGIN REMOTE WRITE` / `END REMOTE WRITE` | Remote write boundaries |
| `ENQUEUED UPDATE` / `SUCCESSFUL UPDATE` / `FAILED UPDATE` | Component update lifecycle |
| `ACTIVATE PLUGIN` / `DEACTIVATE PLUGIN` | Plugin lifecycle — shows plugin name, version, priority |
| `REVERT` | Oplog revert — shows target oplog index |
| `CANCEL INVOCATION` | Cancelled pending invocation — shows idempotency key |
| `START SPAN` / `FINISH SPAN` / `SET SPAN ATTRIBUTE` | Tracing span operations |
| `CHANGE PERSISTENCE LEVEL` | Persistence level change |
| `BEGIN REMOTE TRANSACTION` / `COMMITTED REMOTE TRANSACTION` / `ROLLED BACK REMOTE TRANSACTION` | Remote transaction lifecycle |
| `SNAPSHOT` | Snapshot data — shows mime type and data (JSON or binary size) |
| `OPLOG PROCESSOR CHECKPOINT` | Plugin oplog processor checkpoint — shows plugin, target agent, confirmed/sending indices |
| `SET RETRY POLICY` / `REMOVE RETRY POLICY` | Retry policy changes |
| `RESTART` | Agent restart |
| `INTERRUPTED` / `EXITED` | Agent interrupted or exited |
| `NOP` | No-operation marker |
| `JUMP` | Oplog jump — shows from/to indices |
| `STORAGE USAGE UPDATE` | Filesystem storage usage change |

### Examples

Dump the full oplog:
```shell
golem agent oplog <AGENT_ID>
```

Dump starting from entry 100:
```shell
golem agent oplog <AGENT_ID> --from 100
```

Search for specific entries:
```shell
golem agent oplog <AGENT_ID> --query "error"
```

