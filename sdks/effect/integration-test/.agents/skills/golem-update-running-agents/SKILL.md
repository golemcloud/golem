---
name: golem-update-running-agents
description: "Updating running agents to a new component version. Use when asked to update existing agents, explain update modes (auto/manual), or trigger updates via the CLI or deploy command."
---

# Updating Running Agents

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

When a new version of a component is deployed, existing running agents **continue using their previous version** by default. To move agents to the new version, you must explicitly trigger an update using one of the methods below.

## Update Modes

Golem supports two update modes:

| Mode | CLI Value | Description |
|------|-----------|-------------|
| **Automatic** | `auto` (default) | Replays the agent's operation log against the new component version. Works when the new version is compatible with the old one (same exported functions, compatible signatures). Fails if there is a divergence. |
| **Manual** | `manual` | Uses user-defined `save-snapshot` and `load-snapshot` functions to serialize the agent's state from the old version and restore it in the new version. Required when the new component is incompatible with the old one (changed function signatures, removed functions, restructured state). |

### When to Use Each Mode

- **Use `auto`** when the change is backward-compatible — adding new functions, fixing bugs in existing logic, or changing internal implementation without altering the exported API shape. The operation log can be replayed successfully against the new version.
- **Use `manual`** when the change is breaking — renamed or removed functions, changed parameter types, restructured internal state. The agent's state must be explicitly migrated via snapshot functions.

## Method 1: Update During Deploy

The simplest way to update agents is to pass `--update-agents` (or `-u`) to `golem deploy`:

```shell
golem deploy --yes --update-agents auto
golem deploy --yes --update-agents manual
golem deploy --yes -u auto
```

This deploys the new component version **and** triggers an update for all existing agents of all affected components in one step.

### Other Deploy Strategies

| Flag | Description |
|------|-------------|
| `-u, --update-agents <MODE>` | Update existing agents with `auto` or `manual` mode |
| `--redeploy-agents` | Delete and recreate all existing agents (loses state) |
| `-r, --reset` | Delete agents and the environment, then deploy from scratch |

These flags are mutually exclusive — only one can be used at a time.

## Method 2: Explicit Agent Update Command

Update a single agent to a specific (or latest) component revision:

```shell
golem agent update <AGENT_ID> [MODE] [TARGET_REVISION]
```

### Arguments

| Argument | Description |
|----------|-------------|
| `<AGENT_ID>` | The agent identifier (e.g., `Counter("my-counter")`) |
| `[MODE]` | `auto` (default) or `manual` |
| `[TARGET_REVISION]` | Target component revision number. Defaults to the latest revision if omitted. |

### Flags

| Flag | Description |
|------|-------------|
| `--await` | Wait for the update to complete before returning |
| `--disable-wakeup` | Do not wake up suspended agents; the update is applied next time the agent wakes up |

### Examples

```shell
# Update to latest revision with automatic mode (default)
golem agent update 'Counter("my-counter")'

# Update to latest revision with manual (snapshot-based) mode
golem agent update 'Counter("my-counter")' manual

# Update to a specific revision
golem agent update 'Counter("my-counter")' auto 3

# Wait for the update to finish
golem agent update 'Counter("my-counter")' auto --await

# Don't wake suspended agents
golem agent update 'Counter("my-counter")' auto --disable-wakeup
```

## Method 3: Bulk Update All Agents of a Component

Update all existing agents belonging to a specific component:

```shell
golem component update-agents [OPTIONS]
```

### Arguments

| Argument | Description |
|----------|-------------|
| `--component-name <NAME>` | Component to update agents for |
| `-u, --update-mode <MODE>` | `auto` (default) or `manual` |
| `--await` | Wait for all updates to complete |
| `--disable-wakeup` | Do not wake up suspended agents |

### Examples

```shell
# Update all agents of a component
golem component update-agents --component-name my-component

# With manual mode
golem component update-agents --component-name my-component -u manual --await
```

## Method 4: Application-Level Bulk Update

Update all agents across all components in the current application:

```shell
golem app update-agents [OPTIONS]
```

### Arguments

| Argument | Description |
|----------|-------------|
| `--component-name <NAME>` | Optional filter to specific components (can be repeated) |
| `-u, --update-mode <MODE>` | `auto` (default) or `manual` |
| `--await` | Wait for all updates to complete |
| `--disable-wakeup` | Do not wake up suspended agents |

### Examples

```shell
# Update all agents in the application
golem app update-agents

# Update only agents of specific components
golem app update-agents --component-name comp-a --component-name comp-b -u manual
```

## Method 5: Programmatic Update (Agent-to-Agent)

An agent can trigger an update on another agent programmatically using the Golem host API:

```
update-agent(agent-id, target-revision, mode)
```

This is exposed in each SDK's host bindings. The function returns immediately — it does not wait for the update to complete.

## How Automatic Update Works

1. The agent is interrupted.
2. The agent's operation log (oplog) is replayed from the beginning against the new component version.
3. If replay succeeds, the agent resumes with the new version.
4. If replay fails (e.g., a function no longer exists or has an incompatible signature), the update fails and the agent reverts to the old version.

## How Manual (Snapshot-Based) Update Works

1. The agent is interrupted.
2. The **old** component version's `save-snapshot` export is called, which serializes the agent's state into a byte payload with a MIME type.
3. The agent is restarted with the **new** component version.
4. The new version's `load-snapshot` export is called with the snapshot payload.
5. If `load-snapshot` returns `Ok`, the agent continues with the new version.
6. If `load-snapshot` returns `Err`, the update fails and the agent reverts to the old version.

To implement manual updates, the component must export the `save-snapshot` and `load-snapshot` WIT interfaces. Each SDK provides helpers for this — see the language-specific `golem-custom-snapshot-*` skills for implementation details.


