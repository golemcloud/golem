---
name: golem-recurring-task-scala
description: "Implementing a recurring (cron-like) task in a Scala Golem agent by self-scheduling future invocations. Use when the user asks about periodic tasks, recurring jobs, cron-like scheduling, polling loops, heartbeats, or self-scheduling agents."
---

# Recurring Tasks via Self-Scheduling (Scala)

## Overview

A Golem agent can act as its own scheduler by calling `.poll.scheduleAt(...)` on its own remote client at the end of each invocation. This creates a durable, crash-resilient recurring task — if the agent restarts, the scheduled invocation is still pending and will fire at the designated time.

## Basic Pattern

Use a non-phantom durable agent: `PollerAgentClient.get(name)` must address the
running instance. Call `start` once to do the first poll immediately; subsequent
polls run through the scheduled chain only. Repeated `start` calls or direct
`poll` calls create extra chains. An executor restart recovers the existing
chain and does not require another `start` call. The basic and backoff variants
have no cancel or application-level restart method.

The agent schedules its own method to run again after a delay:

```scala
import golem.Datetime
import golem.BaseAgent
import golem.runtime.annotations.{agentDefinition, agentImplementation}
import scala.concurrent.{ExecutionContext, Future}

given ExecutionContext = ExecutionContext.parasitic

@agentDefinition()
trait PollerAgent extends BaseAgent {
  class Id(val name: String)
  def start(): Future[Unit]
  def poll(): Future[Unit]
}

@agentImplementation()
class PollerAgentImpl(name: String) extends PollerAgent {

  def start(): Future[Unit] = poll()

  def poll(): Future[Unit] = {
    // 1. Do the recurring work
    println(s"Polling $name")

    // 2. Schedule the next run (60 seconds from now)
    val self = PollerAgentClient.get(name)
    self.poll.scheduleAt(Datetime.afterSeconds(60))
  }
}
```

## Exponential Backoff

Replace the basic implementation with this variant. Supply a synchronous `tryWork(): Boolean` helper for the application work; if the work returns a Future, compose it with `flatMap` before scheduling. Increase the delay on repeated failures, reset on success:

```scala
@agentImplementation()
class PollerAgentImpl(name: String) extends PollerAgent {
  private var consecutiveFailures: Int = 0
  private val baseIntervalSecs: Int = 60
  private val maxIntervalSecs: Int = 3600

  def start(): Future[Unit] = poll()

  def poll(): Future[Unit] = {
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

    val self = PollerAgentClient.get(name)
    self.poll.scheduleAt(Datetime.afterSeconds(delay))
  }
}
```

## Cancellation

The cancellation variants use the same single-start contract. Cancellation is
permanent for that agent identity: `start` does not clear `cancelled`, so it
cannot restart the loop. Use a new identity for a new loop. The flag makes an
already queued poll exit without work or rescheduling; it does not undo work
that has already completed.

### Cancellation with CancellationToken

For a durable target with stream-free methods, `scheduleCancelableAt` returns `Future[CancellationToken]`. Compose that future to store the token; do not block with `Await.result` in Scala.js. Add `def cancel(): Future[Unit]` to the trait for this variant:

```scala
import golem.runtime.rpc.CancellationToken

@agentImplementation()
class PollerAgentImpl(name: String) extends PollerAgent {
  private var cancelled: Boolean = false
  private var pendingToken: Option[CancellationToken] = None

  def start(): Future[Unit] = poll()

  def poll(): Future[Unit] =
    if (cancelled) Future.successful(())
    else {
      println(s"Polling $name")
      PollerAgentClient.get(name).poll
        .scheduleCancelableAt(Datetime.afterSeconds(60))
        .map { token => pendingToken = Some(token) }
    }

  def cancel(): Future[Unit] = Future.successful {
    cancelled = true
    pendingToken.foreach(_.cancel())
    pendingToken = None
  }
}
```

### Cancellation via State Flag

For simpler cases, just use a boolean flag — the next scheduled `poll` checks it and exits early:

Keep `start` from the cancellation variant and initialize `private var cancelled = false`.
Replace `poll` and `cancel` with the fragments below, and add the `scheduleNext`
helper shown later. Supply `doWork(): Future[Unit]`; compose it with `flatMap`
before scheduling so its failure does not get discarded.

```scala
def poll(): Future[Unit] =
  if (cancelled) Future.successful(())
  else doWork().flatMap(_ => scheduleNext(60))

def cancel(): Future[Unit] = Future.successful {
  cancelled = true
}
```

### Cancellation from the CLI

This cancels one pending invocation, not the recurring chain. Do not add this
invocation to an already running chain. To stop that chain, invoke the agent's
`cancel` method from one of the cancellation variants above.

For a standalone scheduled poll, use an explicit idempotency key:

```shell
# Schedule with a known idempotency key
golem agent invoke --trigger --schedule-at 2026-03-15T10:30:00Z -i 'poll-next' 'PollerAgent("my-poller")' poll

# Cancel the pending invocation
golem agent cancel-invocation 'PollerAgent("my-poller")' 'poll-next'
```

## Common Use Cases

### Periodic Polling

Check an external API or queue for new work at regular intervals:

```scala
def poll(): Future[Unit] =
  fetchPendingItems().flatMap(items => Future.traverse(items)(process))
    .flatMap(_ => scheduleNext(60))
```

### Periodic Cleanup

Remove expired data or stale resources on a schedule:

```scala
def poll(): Future[Unit] = {
  entries = entries.filterNot(_.isExpired)
  scheduleNext(3600) // run hourly
}
```

### Heartbeat / Keep-Alive

Periodically notify an external service that the agent is alive:

```scala
def poll(): Future[Unit] =
  sendHeartbeat(serviceUrl).flatMap(_ => scheduleNext(30)) // every 30s
```

## Helper for Scheduling Self

These are alternative implementations of the declared `poll` method, so the
helper always schedules `poll`, including cleanup and heartbeat variants.
Supply application helpers with `doWork(): Future[Unit]`,
`fetchPendingItems(): Future[List[Item]]`, `process(item: Item): Future[Unit]`,
and `sendHeartbeat(url: String): Future[Unit]` as applicable.
Extract the scheduling logic into a helper to keep methods clean:

```scala
private def scheduleNext(delaySecs: Int): Future[Unit] = {
  val self = PollerAgentClient.get(name)
  self.poll.scheduleAt(Datetime.afterSeconds(delaySecs))
}
```

## Key Points

- The agent is durable — if it crashes, the pending scheduled invocation still fires and the agent recovers
- Invocations are sequential — no concurrent executions of `poll` on the same agent
- Return or compose each `.scheduleAt` future so the current invocation waits for enqueue acknowledgement, not for the future tick's result
- Use a state flag to stop the loop gracefully
- Keep the scheduled method idempotent — it may be retried on recovery

## Recovery & Oplog Growth

Each scheduled tick (heartbeat, poll, cleanup) appends entries to the agent's oplog. For long-running or high-frequency recurring tasks, the oplog grows unboundedly, and recovery on crash will replay the full history — which becomes slow over time.

**You cannot opt out of oplog writes for a durable agent.** The fix is **snapshot-based recovery**: enable periodic snapshotting so recovery starts from the latest snapshot instead of replaying every prior tick. See [`golem-custom-snapshot-scala`](../golem-custom-snapshot-scala/SKILL.md) for the `@agentDefinition(snapshotting = "every(N)")` / `"periodic(...)"` annotation and `Snapshotted[S]` / custom binary save/load hooks.
