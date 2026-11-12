---
name: golem-debug-agent-history
description: "Querying the operation log. Use when dumping or searching the oplog, debugging agent behavior from its recorded history, or exporting an obfuscated oplog for a bug report."
---

# Debugging Agent History

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

The `<AGENT_ID>` format depends on the agent's language — refer to the language-specific invocation skills for the exact syntax.

To check agent metadata and status, load the `golem-get-agent-metadata` skill. To revert agent state, load the `golem-undo-agent-state` skill.

## `agent oplog` — Query the Operation Log

Stream or search an agent's operation log (oplog). The oplog records every operation the agent has performed — invocations, side effects, persistence boundaries, etc.

```shell
golem agent oplog <AGENT_ID> [OPTIONS]
```

### Flags

| Flag | Description |
|------|-------------|
| `--from <INDEX>` | Start from a specific oplog entry index. Cannot be combined with `--query`. |
| `--query <LUCENE_QUERY>` | Search oplog entries using a Lucene query string. Cannot be combined with `--from`. |
| `--omit-payloads` | Show the entries that oplog pages omit only by their type, timestamp and size. Oplog pages omit an entry whose encoded form is larger than a server limit (1 MiB by default), for example an entry with a large snapshot, invocation input or host call response. Without this flag the CLI reads each omitted entry in full. |
| `--serve` | Serve an interactive HTML viewer on a local port and open it in the default browser. The viewer loads the oplog page by page through the API, runs Lucene queries on the server and can follow a running agent, so it works for oplogs of any size. `--from` and `--query` set the initial position and query. Runs until interrupted. |
| `--port <PORT>` | Port of the served viewer. A free port is chosen when omitted. Requires `--serve`. |
| `--no-open` | Do not open the served viewer in a browser. Requires `--serve`. |
| `--html <FILE>` | Write the selected entries into a self-contained interactive HTML viewer at `FILE` instead of printing them. Every entry is embedded, so prefer `--serve` for large oplogs. Can be combined with `--from` or `--query`. In structured formats the entries are still emitted on stdout. |
| `--open` | Open the file written by `--html` in the default browser. |
| `--obfuscate` | Export an obfuscated report for a bug report: customer values become tokens. Tokens are equal only inside one export, because each CLI process draws a new random key. The report is for reading, not for replay. |
| `--keep-scalars` | Requires `--obfuscate`. Show numbers, bools, datetimes, durations and the mantissa, scale and unit of quantities in clear; never byte lists, discriminants, lengths or text. Leaks amounts, counters, numeric IDs, dates in values, resource IDs, consumed fuel, and every bool and small number that a token hides. |
| `--keep-shape` | Requires `--obfuscate`. Show discriminants, presence, and list, map and byte lengths; the values stay tokens. Leaks whether an optional value was present, which case occurred, item counts and payload sizes; map entries are sorted by value, so their order can show which token stands for which value. |
| `--keep-error-text` | Requires `--obfuscate`. Keep the free text of error messages. Leaks any customer value that the text contains, such as a URL, an input value or a file name. |
| `--max-entry-bytes <N>` | Requires `--obfuscate`. Read an entry that the oplog page omitted only when its size is at most `N` bytes (default 16777216). A larger entry stays omitted with its size. |

If neither `--from` nor `--query` is provided, the entire oplog is streamed.

### Obfuscated export

`--obfuscate` turns the output into a report that is safe to attach to a bug report. Without it, the output is the raw oplog and is not safe to share. Rules:

- The structured output is a sequence of documents: one `agent.oplog-export-header`, one `agent.oplog-export-entry` per entry and one `agent.oplog-export-coverage`, not the `agent.oplog` documents. Branch on `$type`.
- A sequence without a coverage document is from a failed export, and the exit code is not zero.
- Check `incomplete`, `hydrationFailures` and `startUnknown` in the coverage document before you use the report. `incomplete` is `true` when one of the two lists is not empty.
- An omitted `Create` entry above `--max-entry-bytes` fails the export. Export again with a larger limit.
- Agent references are `agent:<token>`, and the report holds only the named agent.
- Tokens are equal only inside one export or one `--serve` session. Do not compare two reports by their tokens.
- The `--keep-*` flags and `--max-entry-bytes` need `--obfuscate`. `--omit-payloads` cannot be used with it. `--serve --obfuscate` works; a served page that answers HTTP 409 means that the agent was created again, so start the viewer again.
- The user who exports reads the raw oplog with the `oplog : read` permission. Obfuscation protects the shared report, not the read.

### Output

Each text entry is printed with its index (e.g. `#00042:`) followed by a labeled header and fields. In structured formats (`json`, `yaml`, `toon`), `agent oplog` emits one output document per entry with `$type: "agent.oplog"`, `index`, `attribution`, and `entry`; parse stdout as a sequence of documents, not as one array or object. The entry types rendered are:

| Entry | Description |
|-------|-------------|
| `CREATE` | Agent creation — shows timestamp, component revision, env vars, parent, initial plugins |
| `START` / `END` / `CANCELLED` | Durable host call — `START` shows the function name, input, and parent start index; `END` shows the result and the start index it completes; `CANCELLED` marks a call that will not complete. Span start, finish, and attributes are shown on these entries |
| `COMPLETION DELIVERED` / `COMPLETION DISCARDED` | Whether a completed host call's result was consumed by the agent |
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
| `ENQUEUED UPDATE` / `SUCCESSFUL UPDATE` / `FAILED UPDATE` | Component update lifecycle |
| `ACTIVATE PLUGIN` / `DEACTIVATE PLUGIN` | Plugin lifecycle — shows plugin name, version, priority |
| `REVERT` | Oplog revert — shows target oplog index |
| `CANCEL INVOCATION` | Cancelled pending invocation — shows idempotency key |
| `BEGIN REMOTE TRANSACTION` / `COMMITTED REMOTE TRANSACTION` / `ROLLED BACK REMOTE TRANSACTION` | Remote transaction lifecycle |
| `SNAPSHOT` | Snapshot data — shows mime type and data (JSON or binary size), and the filesystem snapshot name when the record has one |
| `SNAPSHOT CONFIRMED` | The named filesystem snapshot is in the store — shows the filesystem snapshot name |
| `OPLOG PROCESSOR CHECKPOINT` | Plugin oplog processor checkpoint — shows plugin, target agent, confirmed/sending indices |
| `SET RETRY POLICY` / `REMOVE RETRY POLICY` | Retry policy changes |
| `RESTART` | Agent restart |
| `INTERRUPTED` / `EXITED` | Agent interrupted or exited |
| `JUMP` | Oplog jump — shows from/to indices |
| `CARD INSTALLED` / `CARD DERIVED` / `CARD REVOKED` / `CARD EXPIRED` / `CARD TRANSFER STARTED` / … | Permission card lifecycle |
| `STREAM REGISTERED` / `STREAM ITEMS` / `STREAM END` / `STREAM CANCEL` / `STREAM SESSION` | Streaming invocation events |
| `ENQUEUED EXTERNAL TOOL` / `INVOKE EXTERNAL TOOL` | External tool invocations on the agent |
| `OMITTED ENTRY` | Only with `--omit-payloads`, or when the CLI could not read the full entry: an entry too large for an oplog page — shows its entry type, timestamp and size in bytes. In structured formats its `entry` has `type: "Omitted"` |

### Examples

Stream the full oplog:
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

Explore the oplog in an interactive viewer (grouped by invocation, with foldable host calls, filters, server-side queries, decoded payloads and a follow mode) served locally and opened in the browser:
```shell
golem agent oplog <AGENT_ID> --serve
```

Write the viewer into a self-contained HTML file with the entries embedded, and open it:
```shell
golem agent oplog <AGENT_ID> --html oplog.html --open
```

Export an obfuscated report for a bug report into an HTML file:
```shell
golem agent oplog <AGENT_ID> --obfuscate --html report.html
```

Serve the viewer with each page obfuscated:
```shell
golem agent oplog <AGENT_ID> --obfuscate --serve
```
