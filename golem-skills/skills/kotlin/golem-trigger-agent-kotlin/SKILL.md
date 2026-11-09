---
name: golem-trigger-agent-kotlin
description: "Triggering a fire-and-forget invocation on a Kotlin Golem agent. Use when asked to trigger, enqueue, or fire-and-forget an agent method invocation using golem agent invoke --trigger."
---

# Triggering a Fire-and-Forget Agent Invocation

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## Usage

```shell
golem agent invoke --trigger <AGENT_ID> <FUNCTION_NAME> [ARGUMENTS...]
```

Enqueues the invocation and returns immediately, without waiting for the agent to process it. The agent is automatically created on first invocation if it does not exist yet. Load `golem-invoke-agent-kotlin` for argument encoding, the agent ID format, and Kotlin value syntax — those rules are shared between `golem agent invoke` and its `--trigger` form.

## Example

```shell
golem agent invoke --trigger 'OrderProcessor("order-456")' process
```

The command returns as soon as the host has admitted the invocation; it does not print a result value.

## Idempotency

`-i, --idempotency-key <KEY>` behaves the same as for an awaited invocation: the same key guarantees at-most-once execution, even if the CLI call itself is retried.

## From inside agent code

To fire-and-forget a call to another agent from Kotlin source rather than from the CLI, see `golem-call-another-agent-kotlin`: a generated or typed client's `mTrigger(args)` form, or `WasmRpc.invoke(method, args)`, enqueues the call without blocking for the result.

## Related skills

- `golem-invoke-agent-kotlin` — the awaited form, argument encoding and value syntax
- `golem-schedule-agent-kotlin` — trigger for a specific future time instead of immediately
- `golem-call-another-agent-kotlin` — fire-and-forget from inside a running agent
