---
name: golem-interrupt-resume-agent
description: "Interrupting and resuming a Golem agent. Use when pausing, suspending, interrupting, or resuming an agent instance via the CLI."
---

# Interrupting and Resuming an Agent

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

The `<AGENT_ID>` format depends on the agent's language — refer to the language-specific invocation skills for the exact syntax.

## `agent interrupt` — Pause an Agent

Interrupts a running agent, stopping its execution immediately. The agent's state is preserved — it transitions to the `Interrupted` status and will not process any new invocations until resumed.

```shell
golem agent interrupt <AGENT_ID>
```

### When to Use

- Temporarily pause an agent to prevent it from processing further invocations
- Stop a long-running or misbehaving agent without deleting it
- Perform maintenance — interrupt first, inspect state, then resume

**Note:** Interrupt only works on agents that are actively `Running`. Agents that are `Idle` or `Suspended` (e.g., waiting on a long sleep) cannot be interrupted — they are already paused. Sleeps longer than the configurable suspension threshold (10 seconds by default) cause Golem to suspend the agent automatically; only agents doing active work or shorter sleeps remain in `Running` state.

### Examples

Interrupt a specific agent instance:
```shell
golem agent interrupt CounterAgent("my-counter")
```

Interrupt an agent in a specific environment:
```shell
golem agent interrupt my-env/CounterAgent("my-counter")
```

## `agent resume` — Resume an Interrupted Agent

Resumes an agent that was previously interrupted. The agent transitions back to its normal state and will continue processing invocations, including any that were queued while it was interrupted.

```shell
golem agent resume <AGENT_ID>
```

### Examples

Resume a specific agent instance:
```shell
golem agent resume CounterAgent("my-counter")
```

Resume an agent in a specific environment:
```shell
golem agent resume my-env/CounterAgent("my-counter")
```

## Typical Workflow

1. **Interrupt** the agent to pause execution:
   ```shell
   golem agent interrupt CounterAgent("my-counter")
   ```

2. **Verify** the agent is interrupted (status shows `Interrupted`):
   ```shell
   golem agent get CounterAgent("my-counter")
   ```

3. **Resume** the agent when ready:
   ```shell
   golem agent resume CounterAgent("my-counter")
   ```

4. **Verify** the agent is running again:
   ```shell
   golem agent get CounterAgent("my-counter")
   ```
