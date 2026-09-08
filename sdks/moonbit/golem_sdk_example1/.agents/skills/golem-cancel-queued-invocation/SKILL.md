---
name: golem-cancel-queued-invocation
description: "Canceling a pending (queued) invocation on an agent. Use when canceling, removing, or aborting an enqueued invocation that has not started yet."
---

# Canceling a Queued Invocation

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

The `<AGENT_ID>` format depends on the agent's language — refer to the language-specific invocation skills for the exact syntax.

## Background

Agent invocations are processed sequentially — one at a time. When an invocation arrives while the agent is already busy, it is **enqueued** (queued) and waits until the current invocation finishes. Queued invocations can be canceled before they start processing, using their **idempotency key**.

An idempotency key is a unique string assigned to an invocation. You can set one explicitly when triggering an invocation with `--idempotency-key`, or use `--idempotency-key -` to have Golem auto-generate one. The pending invocation count is visible in the agent's metadata (`golem agent get`).

## `agent cancel-invocation` — Cancel a Queued Invocation

Cancels a pending invocation that has not started yet, identified by its idempotency key.

```shell
golem agent cancel-invocation <AGENT_ID> <IDEMPOTENCY_KEY>
```

- Returns a success message if the invocation was canceled
- If the invocation has already started processing, the cancellation fails (it is too late)

### Examples

Cancel a queued invocation with a known idempotency key:
```shell
golem agent cancel-invocation CounterAgent("my-counter") my-key-123
```

Cancel a queued invocation in a specific environment:
```shell
golem agent cancel-invocation my-env/CounterAgent("my-counter") my-key-123
```

## Typical Workflow

1. **Trigger an invocation** with an explicit idempotency key (so you can reference it later):
   ```shell
   golem agent invoke --trigger --idempotency-key my-batch-job CounterAgent("c1") increment
   ```

2. **Check** that the invocation is pending (pending invocation count > 0):
   ```shell
   golem agent get CounterAgent("c1")
   ```

3. **Cancel** the pending invocation before it starts:
   ```shell
   golem agent cancel-invocation CounterAgent("c1") my-batch-job
   ```

4. **Verify** the invocation was canceled (pending invocation count decreased):
   ```shell
   golem agent get CounterAgent("c1")
   ```
