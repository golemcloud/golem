---
name: golem-schedule-future-call-moonbit
description: "Scheduling a future agent invocation from within MoonBit agent code. Use when the user asks to schedule, delay, or set a timer for calling another agent at a future time."
---

# Scheduling a Future Agent Invocation (MoonBit)

## Overview

A **scheduled invocation** enqueues a method call on the target agent to be executed at a specific future time. The call returns once the target has durably accepted the invocation; the target agent processes it when the scheduled time arrives.

This is for **agent-to-agent scheduled calls from code**. To schedule an invocation from the CLI, see the `golem-schedule-agent-moonbit` skill instead.

## Usage

Methods with recursively stream-free inputs and outputs have a generated async `schedule_` variant. These examples use durable clients. Ephemeral clients have only `upsert` and `scoped_upsert`, and each scheduled invocation runs on a new one-shot instance. Use `AgentClient::scoped_upsert` inside an async function to obtain a client and call `schedule_<method>` with a `scheduled_at` instant and the method arguments. It returns an `@rpc.Invocation[T]` handle, and raises when the invocation was not accepted. Always `drop()` the handle; dropping does not cancel the invocation:

```moonbit
AgentClient::scoped_upsert("param", async fn(client) {
  client.schedule_increment(scheduled_at).drop()
})
```

## Full Example

```moonbit
AgentClient::scoped_upsert("my-counter", async fn(client) {
  // Schedule increment to run 60 seconds from now
  let now = @systemClock.now()
  let scheduled_at = @systemClock.Instant::{
    seconds: now.seconds + 60,
    nanoseconds: 0,
  }
  client.schedule_increment(scheduled_at).drop()
})
```

### Schedule with arguments

```moonbit
ReportAgentClient::scoped_upsert("daily", async fn(client) {
  let scheduled_at = @systemClock.Instant::{
    seconds: tomorrow_midnight,
    nanoseconds: 0,
  }
  client.schedule_generate_report(scheduled_at, "summary").drop()
})
```

## Datetime Type

Import `"golemcloud/golem_sdk/interface/wasi/clocks/system-clock" @systemClock` in `moon.pkg`. The `scheduled_at` parameter is a `@systemClock.Instant` with signed `Int64` seconds and `UInt` nanoseconds since the Unix epoch. `tomorrow_midnight` above must be `Int64`; convert a `UInt64` delay with `.to_int64()` only after checking it fits:

```moonbit
let scheduled_at = @systemClock.Instant::{
  seconds: 1700000000,  // Unix timestamp in seconds
  nanoseconds: 0,       // Sub-second precision
}
```

## Cancellation

The handle that `schedule_` returns cancels the invocation before it starts:

```moonbit
AgentClient::scoped_upsert("my-counter", async fn(client) {
  let now = @systemClock.now()
  let scheduled_at = @systemClock.Instant::{
    seconds: now.seconds + 60,
    nanoseconds: 0,
  }
  let run = client.schedule_increment(scheduled_at)
  defer run.drop()
  // Later, to cancel:
  let withdrawn = run.cancel()
  // ...
})
```

`cancel()` returns `true` when it withdrew the invocation, `false` when it already finished or was already cancelled, and raises `RpcError::CannotCancelRunning` when it is running. `await_result()` on a cancelled invocation raises `RpcError::Cancelled`.

To cancel from a later invocation of the same agent, keep `run.id().to_string()` in agent state and rebuild the handle with the generated static `get_<method>_invocation` (no remote call):

```moonbit
let pending = AgentClient::get_increment_invocation(@rpc.InvocationId::parse(self.pending_run))
defer pending.drop()
let _ = pending.cancel()
```

The ID only locates the invocation; it is not authority. Only the agent that scheduled the invocation, or a caller with management permission, may observe or cancel it. Scheduled calls never lend a scope card.

## Use Cases

- **Periodic tasks**: Schedule the next run at the end of each invocation
- **Delayed processing**: Process an order after a cooling-off period
- **Reminders and notifications**: Send a reminder at a specific time
- **Retry with backoff**: Schedule a retry after a delay on failure
