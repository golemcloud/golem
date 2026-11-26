---
name: golem-recurring-task-moonbit
description: "Implementing a recurring (cron-like) task in a MoonBit Golem agent by self-scheduling future invocations. Use when the user asks about periodic tasks, recurring jobs, cron-like scheduling, polling loops, heartbeats, or self-scheduling agents."
---

# Recurring Tasks via Self-Scheduling (MoonBit)

## Overview

A Golem agent can act as its own scheduler by calling `schedule_` on its own client at the end of each invocation. This creates a durable, crash-resilient recurring task — if the agent restarts, the scheduled invocation is still pending and will fire at the designated time.

## Basic Pattern

The agent schedules its own method to run again after a delay. Import `"golemcloud/golem_sdk/interface/wasi/clocks/system-clock" @systemClock` in `moon.pkg`. Scoped helpers, `schedule_` methods and all their callers are async. `schedule_<method>` returns an `@rpc.Invocation` handle once the target accepted the invocation; drop it when you do not need it (dropping does not cancel the invocation). The work helpers in these fragments must also be async if they perform async I/O:

```moonbit
#derive.agent
struct PollerAgent {
  name : String
}

fn PollerAgent::new(name : String) -> PollerAgent {
  { name }
}

/// Kicks off the first poll
pub async fn PollerAgent::start(self : Self) -> Unit {
  self.poll()
}

/// Does work and schedules itself to run again
pub async fn PollerAgent::poll(self : Self) -> Unit {
  // 1. Do the recurring work
  do_work()

  // 2. Schedule the next run (60 seconds from now)
  PollerAgentClient::scoped_upsert(self.name, async fn(client) {
    let now = @systemClock.now()
    let scheduled_at = @systemClock.Instant::{
      seconds: now.seconds + 60,
      nanoseconds: 0,
    }
    client.schedule_poll(scheduled_at).drop()
  })
}
```

## Exponential Backoff

Call `start` only once per agent. These basic recipes do not support restart or
repeated start: another start creates a second scheduling chain.

Increase the delay on repeated failures, reset on success:

```moonbit
#derive.agent
struct PollerAgent {
  name : String
  mut consecutive_failures : UInt
  base_interval_secs : UInt64
  max_interval_secs : UInt64
}

fn PollerAgent::new(name : String) -> PollerAgent {
  { name, consecutive_failures: 0, base_interval_secs: 60, max_interval_secs: 3600 }
}

pub async fn PollerAgent::poll(self : Self) -> Unit {
  let success = try_work()

  let delay = if success {
    self.consecutive_failures = 0
    self.base_interval_secs
  } else {
    self.consecutive_failures += 1
    let exp = if self.consecutive_failures > 6 { 6U } else { self.consecutive_failures }
    let backoff = self.base_interval_secs * pow2(exp)
    if backoff > self.max_interval_secs { self.max_interval_secs } else { backoff }
  }

  PollerAgentClient::scoped_upsert(self.name, async fn(client) {
    let now = @systemClock.now()
    client.schedule_poll(@systemClock.Instant::{
      seconds: now.seconds + delay.to_int64(),
      nanoseconds: 0,
    }).drop()
  })
}
```

## Cancellation

### Cancellation with the Invocation Handle

`schedule_<method>` returns an `@rpc.Invocation` handle whose `cancel()` withdraws the scheduled invocation before it starts. The handle is a host resource and is not snapshot-serializable, so store the text of its ID in state and rebuild the handle with the generated static `get_<method>_invocation` when cancelling:

```moonbit
#derive.agent
struct PollerAgent {
  name : String
  mut cancelled : Bool
  mut pending_tick : String?
}

fn PollerAgent::new(name : String) -> PollerAgent {
  { name, cancelled: false, pending_tick: None }
}

pub async fn PollerAgent::poll(self : Self) -> Unit {
  if self.cancelled {
    return
  }
  do_work()
  PollerAgentClient::scoped_upsert(self.name, async fn(client) {
    let now = @systemClock.now()
    let next = client.schedule_poll(@systemClock.Instant::{
      seconds: now.seconds + 60,
      nanoseconds: 0,
    })
    self.pending_tick = Some(next.id().to_string())
    next.drop()
  })
}

pub async fn PollerAgent::cancel(self : Self) -> Unit {
  self.cancelled = true
  match self.pending_tick {
    Some(text) => {
      let tick = PollerAgentClient::get_poll_invocation(
        @rpc.InvocationId::parse(text),
      )
      defer tick.drop()
      let _ = tick.cancel()
      self.pending_tick = None
    }
    None => ()
  }
}
```

`cancel()` returns `true` when it withdrew the tick and `false` when the tick already finished or was already cancelled. It raises `RpcError::CannotCancelRunning` for a running invocation; the agent runs one invocation at a time, so its own pending tick is not running while `cancel` runs. The agent that scheduled the tick may cancel it; other callers need management permission. Keep the `cancelled` flag as well: a tick whose ID was not stored, for example one scheduled from the CLI, still exits without doing work.

### Cancellation via State Flag

For simpler cases, use a boolean flag — the next scheduled `poll` checks it and exits early. Cancellation is permanent: never reset the flag or restart this chain while an old tick may still be pending:

```moonbit
pub async fn PollerAgent::poll(self : Self) -> Unit {
  if self.cancelled {
    return
  }
  do_work()
  self.schedule_next(60)
}

pub fn PollerAgent::cancel(self : Self) -> Unit {
  self.cancelled = true
}
```

### Cancellation from the CLI

Schedule with an explicit idempotency key and cancel the pending invocation:

```shell
# Schedule with a known idempotency key
golem agent invoke --trigger --schedule-at 2026-03-15T10:30:00Z -i 'poll-next' 'PollerAgent("my-poller")' poll

# Cancel the pending invocation
golem invocation cancel --agent 'PollerAgent("my-poller")' --idempotency-key 'poll-next'
```

## Common Use Cases

### Periodic Polling

Check an external API or queue for new work at regular intervals:

```moonbit
pub async fn PollerAgent::poll(self : Self) -> Unit {
  let items = fetch_pending_items()
  for item in items {
    process(item)
  }
  self.schedule_next(60)
}
```

### Periodic Cleanup

Remove expired data or stale resources on a schedule:

```moonbit
pub async fn PollerAgent::poll(self : Self) -> Unit {
  self.entries = self.entries.filter(fn(e) { !e.is_expired() })
  self.schedule_next(3600) // run hourly
}
```

### Heartbeat / Keep-Alive

Periodically notify an external service that the agent is alive:

```moonbit
pub async fn PollerAgent::poll(self : Self) -> Unit {
  send_heartbeat(self.service_url)
  self.schedule_next(30) // every 30s
}
```

## Helper for Scheduling Self

Extract the scheduling logic into a helper to keep methods clean:

```moonbit
async fn PollerAgent::schedule_next(self : Self, delay_secs : Int64) -> Unit {
  PollerAgentClient::scoped_upsert(self.name, async fn(client) {
    let now = @systemClock.now()
    client.schedule_poll(@systemClock.Instant::{
      seconds: now.seconds + delay_secs,
      nanoseconds: 0,
    }).drop()
  })
}
```

## Key Points

- The agent is durable — if it crashes, the pending scheduled invocation still fires and the agent recovers
- Invocations are sequential — no concurrent executions of `poll` on the same agent
- Each `schedule_` call waits only until the target accepted the invocation; the current invocation continues and completes when its method returns
- Use a state flag to stop the loop gracefully
- Keep the scheduled method idempotent — it may be retried on recovery

## Recovery & Oplog Growth

Each scheduled tick (heartbeat, poll, cleanup) appends entries to the agent's oplog. For long-running or high-frequency recurring tasks, the oplog grows unboundedly, and recovery on crash will replay the full history — which becomes slow over time.

**You cannot opt out of oplog writes for a durable agent.** The fix is **snapshot-based recovery**: enable periodic snapshotting so recovery starts from the latest snapshot instead of replaying every prior tick. See [`golem-custom-snapshot-moonbit`](../golem-custom-snapshot-moonbit/SKILL.md) for the `#derive.agent(snapshotting="every_n(N)")` attribute and the `Snapshottable` trait.

For snapshots, keep only serializable state: store a pending tick as the text of
its invocation ID, never as an `@rpc.Invocation` handle. The cleanup and heartbeat snippets are alternative `poll`
bodies and therefore use the same `schedule_next` helper. For restart support,
store a serializable generation counter and pass it to every scheduled tick;
reject ticks from an older generation before doing work or scheduling again.
