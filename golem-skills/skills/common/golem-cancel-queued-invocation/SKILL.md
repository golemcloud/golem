---
name: golem-cancel-queued-invocation
description: "Inspecting and canceling a pending (queued) or scheduled invocation on an agent. Use when checking the state or result of an invocation, listing an agent's invocations, or canceling, removing, or aborting an invocation that has not started yet."
---

# Inspecting and Canceling Invocations

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

The `<AGENT_ID>` format depends on the agent's language — refer to the language-specific invocation skills for the exact syntax.

## Background

Agent invocations are processed sequentially — one at a time. When an invocation arrives while the agent is already busy, it is **enqueued** (queued) and waits until the current invocation finishes. A **scheduled** invocation waits until its scheduled time. Pending and scheduled invocations can be canceled before they start processing.

An invocation is selected in one of two ways:

- by its **agent and idempotency key**: set a key explicitly when triggering an invocation with `--idempotency-key`, or use `--idempotency-key -` to have Golem generate one;
- by its **invocation ID**, `<agent ID>.<UUID>`, as printed by `golem invocation list`.

The same idempotency key used for more than one method of the agent selects one invocation per method. `invocation get` shows all of them, each with its operation; `invocation cancel` needs `--method <METHOD>` to choose one, using the method name as shown in the Operation column of `golem invocation list`.

A key is found only for the account that made the invocation, and keys of invocations made through an HTTP route are never found by key. An invocation ID is resolved in the current environment; select another one with `--environment`.

## `invocation get` — Show an Invocation

```shell
golem invocation get --agent <AGENT_ID> --idempotency-key <IDEMPOTENCY_KEY>
golem invocation get <INVOCATION_ID>
```

Shows the invocation's state (`scheduled`, `pending`, `running`, `waiting`, `retrying`, `interrupted`, `completed`, `failed` or `cancelled`), its operation and timestamps, and the result of a `completed` or `failed` invocation. The command exits with code **130** when the invocation, or one of the invocations that the key selects, was cancelled.

## `invocation list` — List the Invocations of an Agent

```shell
golem invocation list --agent <AGENT_ID>
```

Lists invocations newest change first. Filter with `--operation <METHOD>`, `--status <STATE>` (repeatable), `--kind method|constructor|tool`, `--since <RFC 3339 time>` and `--until <RFC 3339 time>`. Pages hold at most `--page-size` invocations (default 100); pass the printed cursor with `--cursor` to get the next page.

## `invocation cancel` — Cancel a Pending or Scheduled Invocation

```shell
golem invocation cancel --agent <AGENT_ID> --idempotency-key <IDEMPOTENCY_KEY>
golem invocation cancel <INVOCATION_ID>
```

- Prints the invocation after the cancel: its state is `cancelled` when the cancel took effect
- An invocation that already finished keeps its state and result
- A running invocation cannot be canceled; the command fails

### Examples

Cancel a queued invocation with a known idempotency key:
```shell
golem invocation cancel --agent 'CounterAgent("my-counter")' --idempotency-key my-key-123
```

Cancel a queued invocation in a specific environment:
```shell
golem invocation cancel --agent 'my-env/CounterAgent("my-counter")' --idempotency-key my-key-123
```

## Typical Workflow

1. **Trigger an invocation** with an explicit idempotency key (so you can reference it later):
   ```shell
   golem agent invoke --trigger --idempotency-key my-batch-job 'CounterAgent("c1")' increment
   ```

2. **Check** that the invocation is still pending:
   ```shell
   golem invocation get --agent 'CounterAgent("c1")' --idempotency-key my-batch-job
   ```

3. **Cancel** the pending invocation before it starts:
   ```shell
   golem invocation cancel --agent 'CounterAgent("c1")' --idempotency-key my-batch-job
   ```

4. **Verify** the invocation was canceled (`get` reports `cancelled` and exits with code 130):
   ```shell
   golem invocation get --agent 'CounterAgent("c1")' --idempotency-key my-batch-job
   ```
