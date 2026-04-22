---
name: golem-test-crash-recovery
description: "Simulating a crash on an agent for testing crash recovery. Use when testing durable execution, verifying state recovery after failures, or validating that an agent correctly resumes from its operation log."
---

# Testing Crash Recovery

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

The `<AGENT_ID>` format depends on the agent's language — refer to the language-specific invocation skills for the exact syntax.

## `agent simulate-crash` — Simulate a Crash

Simulates a crash on an agent for testing purposes. The agent starts recovering and resuming immediately by replaying its operation log.

This is useful for verifying that an agent's durable state is correctly persisted and that it can recover from unexpected failures without data loss.

```shell
golem agent simulate-crash <AGENT_ID>
```

### What Happens

1. The agent's execution is interrupted (simulating a process crash)
2. The agent immediately begins recovery by replaying its operation log
3. Once recovery completes, the agent resumes normal operation

### When to Use

- **Testing durability**: Verify that agent state survives crashes and is correctly restored
- **Validating transactions**: Ensure saga-pattern compensations or atomic blocks behave correctly after a crash
- **Debugging recovery issues**: Reproduce crash-recovery scenarios to diagnose problems with state restoration

### Examples

Simulate a crash on an agent:
```shell
golem agent simulate-crash <AGENT_ID>
```

Typical test workflow — invoke a method, simulate a crash, then verify state is preserved:
```shell
golem agent invoke <AGENT_ID> <METHOD> [ARGS]
golem agent simulate-crash <AGENT_ID>
golem agent invoke <AGENT_ID> <GETTER_METHOD>
```
