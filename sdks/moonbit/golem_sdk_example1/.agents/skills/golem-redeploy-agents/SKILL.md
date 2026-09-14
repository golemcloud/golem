---
name: golem-redeploy-agents
description: "Redeploying existing agents by deleting and recreating them. Use when asked to redeploy agents, reset agent state while keeping the same identity, or force agents to start fresh with the latest component version."
---

# Redeploying Existing Agents

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## Overview

Redeploying an agent means **deleting** it and **recreating** it with the same name, environment variables, and configuration, but using the latest deployed component version. Unlike an in-place update, redeployment destroys all agent state — the operation log (oplog), internal WASM state, and any data stored in Golem's key-value storage for that agent are permanently lost. The agent starts fresh as if it were newly created.

Use redeployment when:

- The agent's persisted state is incompatible with the new component version and cannot be migrated via snapshot-based updates.
- You want to force a clean start for all agents without manually deleting and recreating them one by one.
- During iterative development when preserving agent state is not important.

If you need to preserve agent state across component version changes, use **update** (`--update-agents`) instead — see the `golem-update-running-agents` skill.

## Method 1: Redeploy During Deploy

The simplest approach — pass `--redeploy-agents` to `golem deploy`:

```shell
golem deploy --yes --redeploy-agents
```

This deploys the new component version and then, for every existing agent of each affected component:

1. Lists all agents of the component.
2. Deletes each agent (destroying its oplog and stored state).
3. Recreates each agent with the same name, environment variables, and WASI configuration, pointing to the latest component version.

### Comparison with Other Post-Deploy Flags

| Flag | Behavior |
|------|----------|
| `--redeploy-agents` | Delete and recreate agents (loses all state) |
| `--update-agents <MODE>` | Update agents in-place via oplog replay (`auto`) or snapshots (`manual`) — preserves state |
| `--reset` | Delete agents **and** the environment, then deploy from scratch |

These flags are mutually exclusive — only one can be used at a time. If both `--redeploy-agents` and `--reset` are specified, `--reset` takes precedence and agents are simply deleted (not recreated).

## Method 2: Environment-Level Default

You can configure `--redeploy-agents` as the default behavior for an environment in the application manifest (`golem.yaml`). This is useful during development so you don't have to pass the flag every time:

```yaml
environments:
  local:
    cli:
      redeployAgents: true
```

With this configuration, every `golem deploy` targeting the `local` environment will automatically redeploy agents. The CLI `--redeploy-agents` flag overrides or supplements this setting.

## What Gets Preserved

| Aspect | Preserved? |
|--------|------------|
| Agent name (identity) | ✅ Yes |
| Environment variables | ✅ Yes |
| WASI configuration | ✅ Yes |
| Component configuration | ✅ Yes |
| Component version | ❌ No — upgraded to latest |
| Operation log (oplog) | ❌ No — deleted |
| Internal WASM state | ❌ No — reset |
| KV storage data | ❌ No — deleted |

## When to Use Redeploy vs. Update vs. Reset

| Scenario | Recommended Approach |
|----------|---------------------|
| Iterative development, state doesn't matter | `--redeploy-agents` or `--reset` |
| Backward-compatible code change, preserve state | `--update-agents auto` |
| Breaking change with snapshot migration implemented | `--update-agents manual` |
| Breaking change, no snapshot migration, state expendable | `--redeploy-agents` |
| Start completely fresh (new environment) | `--reset` |

## Example Workflow

```shell
# 1. Make code changes to your component

# 2. Deploy and redeploy all agents
golem deploy --yes --redeploy-agents

# 3. Verify agents are running with the new version
golem agent list --component-name my-component
```

## Notes

- Redeployment requires confirmation in interactive mode. Always pass `--yes` to skip the prompt.
- If no agents exist for a component, the redeploy step is skipped with a warning.
- Redeployment is performed sequentially — agents are deleted and recreated one at a time.
