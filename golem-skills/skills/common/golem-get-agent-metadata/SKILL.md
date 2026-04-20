---
name: golem-get-agent-metadata
description: "Checking agent metadata and status. Use when inspecting an agent's state, component revision, environment variables, update history, or current status."
---

# Getting Agent Metadata

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

The `<AGENT_ID>` format depends on the agent's language — refer to the language-specific invocation skills for the exact syntax.

## `agent get` — Check Agent Metadata

```shell
golem agent get <AGENT_ID>
```

Displays the agent's current metadata including:

- Component name and revision
- Agent name
- Creation timestamp
- Component size and total linear memory size
- Environment variables (if any)
- Status (`Running`, `Idle`, `Suspended`, `Interrupted`, `Retrying`, `Failed`, `Exited`) — shown when `--precise` is used
- Retry count — shown when `--precise` is used
- Pending invocation count (if > 0)
- Last error (if any, when `--precise` is used)
- Update history (pending, successful, and failed updates)

Note: by default, metadata may not be fully up-to-date. The `agent list --precise` flag queries for the most current status.
