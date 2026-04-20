---
name: golem-schedule-future-call-moonbit
description: "Scheduling a future agent invocation from within MoonBit agent code. Use when the user asks to schedule, delay, or set a timer for calling another agent at a future time."
---

# Scheduling a Future Agent Invocation (MoonBit)

## Overview

A **scheduled invocation** enqueues a method call on the target agent to be executed at a specific future time. The call returns immediately; the target agent processes it when the scheduled time arrives.

This is for **agent-to-agent scheduled calls from code**. To schedule an invocation from the CLI, see the `golem-schedule-agent-moonbit` skill instead.

## Usage

Every method on the generated `AgentClient` has a corresponding `schedule_` variant. Use `AgentClient::scoped` to obtain a client and call the schedule method with a `scheduled_at` datetime and the method arguments:

```moonbit
AgentClient::scoped("param", fn(client) raise @common.AgentError {
  client.schedule_increment(scheduled_at)
})
```

## Full Example

```moonbit
AgentClient::scoped("my-counter", fn(client) raise @common.AgentError {
  // Schedule increment to run 60 seconds from now
  let now = @wallClock.now()
  let scheduled_at = @wallClock.Datetime::{
    seconds: now.seconds + 60,
    nanoseconds: 0,
  }
  client.schedule_increment(scheduled_at)
})
```

### Schedule with arguments

```moonbit
ReportAgentClient::scoped("daily", fn(client) raise @common.AgentError {
  let scheduled_at = @wallClock.Datetime::{
    seconds: tomorrow_midnight,
    nanoseconds: 0,
  }
  client.schedule_generate_report(scheduled_at, "summary")
})
```

## Datetime Type

The `scheduled_at` parameter is a `@wallClock.Datetime` value representing a point in time as seconds + nanoseconds since the Unix epoch:

```moonbit
let scheduled_at = @wallClock.Datetime::{
  seconds: 1700000000,  // Unix timestamp in seconds
  nanoseconds: 0,       // Sub-second precision
}
```

## Use Cases

- **Periodic tasks**: Schedule the next run at the end of each invocation
- **Delayed processing**: Process an order after a cooling-off period
- **Reminders and notifications**: Send a reminder at a specific time
- **Retry with backoff**: Schedule a retry after a delay on failure
