---
name: golem-schedule-future-call-scala
description: "Scheduling a future agent invocation in a Scala Golem project. Use when the user asks about delayed invocations, scheduling calls for later, or timed agent execution."
---

# Scheduling a Future Agent Invocation (Scala)

## Overview

A **scheduled invocation** enqueues a method call on the target agent to be executed at a specific future time. The call returns immediately; the target agent processes it when the scheduled time arrives.

## Usage

Access the `.schedule` proxy on the client and pass a `Datetime` as the first argument:

```scala
import golem.Datetime

val counter = CounterAgent.get("my-counter")

// Schedule increment to run 5 seconds from now
counter.schedule.increment(Datetime.afterSeconds(5))

// Schedule with arguments
val reporter = ReportAgent.get("daily")
reporter.schedule.generateReport(
  Datetime.afterSeconds(3600), // 1 hour from now
  "summary"
)
```

## Datetime Helper

The `golem.Datetime` type provides helper methods:

```scala
import golem.Datetime

Datetime.afterSeconds(60)     // 60 seconds from now
Datetime.afterSeconds(3600)   // 1 hour from now
```

## Use Cases

- **Periodic tasks**: Schedule the next run at the end of each invocation
- **Delayed processing**: Process an order after a cooling-off period
- **Reminders and notifications**: Send a reminder at a specific time
- **Retry with backoff**: Schedule a retry after a delay on failure
