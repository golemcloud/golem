---
name: golem-recurring-task-scala
description: "Implementing a recurring (cron-like) task in a Scala Golem agent by self-scheduling future invocations. Use when the user asks about periodic tasks, recurring jobs, cron-like scheduling, polling loops, heartbeats, or self-scheduling agents."
---

# Recurring Tasks via Self-Scheduling (Scala)

## Overview

A Golem agent can act as its own scheduler by calling `.poll.scheduleAt(...)` on its own remote client at the end of each invocation. This creates a durable, crash-resilient recurring task — if the agent restarts, the scheduled invocation is still pending and will fire at the designated time.

## Basic Pattern

The agent schedules its own method to run again after a delay:

```scala
import golem.Datetime

@agentDefinition
trait PollerAgent extends BaseAgent {
  class Id(name: String) derives Schema
  def start(): Unit
  def poll(): Unit
}

@agentImplementation()
class PollerAgentImpl(id: PollerAgent.Id) extends PollerAgent {

  def start(): Unit = poll()

  def poll(): Unit = {
    // 1. Do the recurring work
    doWork()

    // 2. Schedule the next run (60 seconds from now)
    val self = PollerAgentClient.get(id.name)
    self.poll.scheduleAt(Datetime.afterSeconds(60))
  }
}
```

## Exponential Backoff

Increase the delay on repeated failures, reset on success:

```scala
@agentImplementation()
class PollerAgentImpl(id: PollerAgent.Id) extends PollerAgent {
  private var consecutiveFailures: Int = 0
  private val baseIntervalSecs: Int = 60
  private val maxIntervalSecs: Int = 3600

  def poll(): Unit = {
    val success = tryWork()

    val delay = if (success) {
      consecutiveFailures = 0
      baseIntervalSecs
    } else {
      consecutiveFailures += 1
      val exp = Math.min(consecutiveFailures, 6)
      val backoff = baseIntervalSecs * Math.pow(2, exp).toInt
      Math.min(backoff, maxIntervalSecs)
    }

    val self = PollerAgentClient.get(id.name)
    self.poll.scheduleAt(Datetime.afterSeconds(delay))
  }
}
```

## Cancellation

### Cancellation with CancellationToken

Every generated remote method has a `scheduleCancelableAt` variant that returns a `Future[CancellationToken]`. Store the token and call `.cancel()` to prevent the scheduled invocation from firing:

```scala
import golem.runtime.rpc.CancellationToken

@agentImplementation()
class PollerAgentImpl(id: PollerAgent.Id) extends PollerAgent {
  private var cancelled: Boolean = false
  private var pendingToken: Option[CancellationToken] = None

  def poll(): Unit = {
    if (cancelled) return

    doWork()

    val self = PollerAgentClient.get(id.name)
    pendingToken = Some(Await.result(
      self.poll.scheduleCancelableAt(Datetime.afterSeconds(60)),
      Duration.Inf
    ))
  }

  def cancel(): Unit = {
    cancelled = true
    pendingToken.foreach(_.cancel())
    pendingToken = None
  }
}
```

### Cancellation via State Flag

For simpler cases, just use a boolean flag — the next scheduled `poll` checks it and exits early:

```scala
def poll(): Unit = {
  if (cancelled) return
  doWork()
  scheduleNext(60)
}

def cancel(): Unit = {
  cancelled = true
}
```

### Cancellation from the CLI

Schedule with an explicit idempotency key and cancel the pending invocation:

```shell
# Schedule with a known idempotency key
golem agent invoke --trigger --schedule-at 2026-03-15T10:30:00Z -i 'poll-next' 'PollerAgent("my-poller")' poll

# Cancel the pending invocation
golem agent invocation cancel 'PollerAgent("my-poller")' 'poll-next'
```

## Common Use Cases

### Periodic Polling

Check an external API or queue for new work at regular intervals:

```scala
def poll(): Unit = {
  val items = fetchPendingItems()
  items.foreach(process)
  scheduleNext(60)
}
```

### Periodic Cleanup

Remove expired data or stale resources on a schedule:

```scala
def cleanup(): Unit = {
  entries = entries.filterNot(_.isExpired)
  scheduleNext(3600) // run hourly
}
```

### Heartbeat / Keep-Alive

Periodically notify an external service that the agent is alive:

```scala
def heartbeat(): Unit = {
  sendHeartbeat(serviceUrl)
  scheduleNext(30) // every 30s
}
```

## Helper for Scheduling Self

Extract the scheduling logic into a helper to keep methods clean:

```scala
private def scheduleNext(delaySecs: Int): Unit = {
  val self = PollerAgentClient.get(id.name)
  self.poll.scheduleAt(Datetime.afterSeconds(delaySecs))
}
```

## Key Points

- The agent is durable — if it crashes, the pending scheduled invocation still fires and the agent recovers
- Invocations are sequential — no concurrent executions of `poll` on the same agent
- Each `.scheduleAt` call is a fire-and-forget enqueue; the current invocation completes immediately
- Use a state flag to stop the loop gracefully
- Keep the scheduled method idempotent — it may be retried on recovery
