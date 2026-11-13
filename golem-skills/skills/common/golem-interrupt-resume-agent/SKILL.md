---
name: golem-interrupt-resume-agent
description: "Interrupting and resuming a Golem agent. Use when pausing, suspending, interrupting, or resuming an agent instance via the CLI."
---

# Interrupting and Resuming an Agent

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

The `<AGENT_ID>` format depends on the agent's language — refer to the language-specific invocation skills for the exact syntax.

## `agent interrupt` — Pause an Agent

Interrupts a running agent, stopping its execution. A durable agent preserves its state and transitions to `Interrupted`. This is not a maintenance lock: a new invocation can start an interrupted durable agent without an explicit resume. Stop incoming invocations separately if the agent must remain interrupted.

```shell
golem agent interrupt <AGENT_ID>
```

### When to Use

- Stop the current execution of an agent
- Stop a long-running or misbehaving agent without deleting it
- Inspect a durable agent's state after stopping incoming invocations and interrupting it

**Note:** Agents in `Running`, `Suspended`, or `Retrying` can be interrupted. Requests for `Idle`, `Failed`, `Exited`, or already `Interrupted` agents are ignored. Suspension (for example during a long sleep) is automatic and does not prevent interruption.

### Examples

Interrupt a specific agent instance:
```shell
golem agent interrupt 'CounterAgent("my-counter")'
```

Interrupt an agent in a specific environment:
```shell
golem agent interrupt 'my-env/CounterAgent("my-counter")'
```

## `agent resume` — Resume an Interrupted Agent

Resumes a durable agent that was previously interrupted, continuing its persisted execution. Explicit resume fails for ephemeral agents. After resume, the status depends on the work: it can be `Running`, `Idle`, or `Suspended`.

```shell
golem agent resume <AGENT_ID>
```

### Examples

Resume a specific agent instance:
```shell
golem agent resume 'CounterAgent("my-counter")'
```

Resume an agent in a specific environment:
```shell
golem agent resume 'my-env/CounterAgent("my-counter")'
```

## Typical Workflow (Durable Agents)

First stop incoming invocations if the agent must stay interrupted during inspection.

1. **Interrupt** the agent to pause execution:
   ```shell
   golem agent interrupt 'CounterAgent("my-counter")'
   ```

2. **Verify** the agent is interrupted (status shows `Interrupted`):
   ```shell
   golem agent get 'CounterAgent("my-counter")'
   ```

3. **Resume** the agent when ready:
   ```shell
   golem agent resume 'CounterAgent("my-counter")'
   ```

4. **Verify** the agent has resumed; expect `Running`, `Idle`, or `Suspended` depending on its work:
   ```shell
   golem agent get 'CounterAgent("my-counter")'
   ```
