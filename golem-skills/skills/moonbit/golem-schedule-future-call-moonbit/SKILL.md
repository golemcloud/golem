---
name: golem-schedule-future-call-moonbit
description: "Scheduling a future agent invocation from within MoonBit agent code. Use when the user asks to schedule, delay, or set a timer for calling another agent at a future time."
---

# Scheduling a Future Agent Invocation (MoonBit)

## Overview

A **scheduled invocation** enqueues a method call on the target agent to be executed at a specific future time. The call returns immediately; the target agent processes it when the scheduled time arrives.

This is for **agent-to-agent scheduled calls from code**. To schedule an invocation from the CLI, see the `golem-schedule-agent-moonbit` skill instead.

## Usage

Methods with recursively stream-free inputs and outputs have a generated `schedule_` variant. These examples use durable clients. Ephemeral clients require `scoped_new_phantom` or `scoped_get_phantom`, not `scoped`. Use `AgentClient::scoped` inside an async function to obtain a client and call the synchronous enqueue method with a `scheduled_at` instant and the method arguments. The scoped helper and its callback are async even though scheduling itself is synchronous:

```moonbit
AgentClient::scoped("param", async fn(client) {
  client.schedule_increment(scheduled_at)
})
```

## Full Example

```moonbit
AgentClient::scoped("my-counter", async fn(client) {
  // Schedule increment to run 60 seconds from now
  let now = @systemClock.now()
  let scheduled_at = @systemClock.Instant::{
    seconds: now.seconds + 60,
    nanoseconds: 0,
  }
  client.schedule_increment(scheduled_at)
})
```

### Schedule with arguments

```moonbit
ReportAgentClient::scoped("daily", async fn(client) {
  let scheduled_at = @systemClock.Instant::{
    seconds: tomorrow_midnight,
    nanoseconds: 0,
  }
  client.schedule_generate_report(scheduled_at, "summary")
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

## Cancelable Variant

Stream-free methods on durable clients also have a `schedule_cancelable_` variant that returns a `CancellationToken`. Ephemeral clients return a receipt containing the token and invocation metadata instead. Call `.cancel()` on the token to prevent the scheduled invocation from firing:

```moonbit
AgentClient::scoped("my-counter", async fn(client) {
  let now = @systemClock.now()
  let scheduled_at = @systemClock.Instant::{
    seconds: now.seconds + 60,
    nanoseconds: 0,
  }
  let token = client.schedule_cancelable_increment(scheduled_at)
  defer token.drop()
  // Later, to cancel: token.cancel()
  // cancel() borrows the token; defer releases it whether cancelled or not.
})
```

## Use Cases

- **Periodic tasks**: Schedule the next run at the end of each invocation
- **Delayed processing**: Process an order after a cooling-off period
- **Reminders and notifications**: Send a reminder at a specific time
- **Retry with backoff**: Schedule a retry after a delay on failure
