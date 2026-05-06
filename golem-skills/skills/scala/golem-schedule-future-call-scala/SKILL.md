---
name: golem-schedule-future-call-scala
description: "Scheduling a future agent invocation in a Scala Golem project. Use when the user asks about delayed invocations, scheduling calls for later, or timed agent execution."
---

# Scheduling a Future Agent Invocation (Scala)

## Overview

A **scheduled invocation** enqueues a method call on the target agent to be executed at a specific future time. The call returns immediately; the target agent processes it when the scheduled time arrives.

## Usage

Each method on the remote proxy has a `.scheduleAt(when)` method that enqueues the call for the given time:

```scala
import golem.Datetime

val counter = CounterAgentClient.get("my-counter")

// Schedule increment to run 5 seconds from now
counter.increment.scheduleAt(Datetime.afterSeconds(5))

// Schedule with arguments — method params first, then when
val reporter = ReportAgentClient.get("daily")
reporter.generateReport.scheduleAt(
  "summary",
  when = Datetime.afterSeconds(3600) // 1 hour from now
)
```

## Datetime Helper

The `golem.Datetime` type provides helper methods:

```scala
import golem.Datetime

Datetime.afterSeconds(60)     // 60 seconds from now
Datetime.afterSeconds(3600)   // 1 hour from now
```

## Cancelable Variant

Every method also has a `scheduleCancelableAt` variant that returns a `Future[CancellationToken]`. Call `.cancel()` on the token to prevent the scheduled invocation from firing:

```scala
import golem.runtime.rpc.CancellationToken

val token: CancellationToken = Await.result(
  counter.increment.scheduleCancelableAt(Datetime.afterSeconds(60)),
  Duration.Inf
)

// Later, to cancel the pending invocation:
token.cancel()
```

## Use Cases

- **Periodic tasks**: Schedule the next run at the end of each invocation
- **Delayed processing**: Process an order after a cooling-off period
- **Reminders and notifications**: Send a reminder at a specific time
- **Retry with backoff**: Schedule a retry after a delay on failure
