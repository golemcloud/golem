---
name: golem-recurring-task-moonbit
description: "Implementing a recurring (cron-like) task in a MoonBit Golem agent by self-scheduling future invocations. Use when the user asks about periodic tasks, recurring jobs, cron-like scheduling, polling loops, heartbeats, or self-scheduling agents."
---

# Recurring Tasks via Self-Scheduling (MoonBit)

## Overview

A Golem agent can act as its own scheduler by calling `schedule_` on its own client at the end of each invocation. This creates a durable, crash-resilient recurring task — if the agent restarts, the scheduled invocation is still pending and will fire at the designated time.

## Basic Pattern

The agent schedules its own method to run again after a delay:

```moonbit
#derive.agent
struct PollerAgent {
  name : String
}

fn PollerAgent::new(name : String) -> PollerAgent {
  { name }
}

/// Kicks off the first poll
pub fn PollerAgent::start(self : Self) -> Unit {
  self.poll()
}

/// Does work and schedules itself to run again
pub fn PollerAgent::poll(self : Self) -> Unit {
  // 1. Do the recurring work
  do_work()

  // 2. Schedule the next run (60 seconds from now)
  PollerAgentClient::scoped(self.name, fn(client) raise @common.AgentError {
    let now = @wallClock.now()
    let scheduled_at = @wallClock.Datetime::{
      seconds: now.seconds + 60,
      nanoseconds: 0,
    }
    client.schedule_poll(scheduled_at)
  })
}
```

## Exponential Backoff

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

pub fn PollerAgent::poll(self : Self) -> Unit {
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

  PollerAgentClient::scoped(self.name, fn(client) raise @common.AgentError {
    let now = @wallClock.now()
    client.schedule_poll(@wallClock.Datetime::{
      seconds: now.seconds + delay.to_uint64(),
      nanoseconds: 0,
    })
  })
}
```

## Cancellation

### Cancellation with CancellationToken

Every generated client has `schedule_cancelable_{method}` variants that return a `CancellationToken`. Call `.cancel()` on the token to prevent the scheduled invocation from firing:

```moonbit
#derive.agent
struct PollerAgent {
  name : String
  mut cancelled : Bool
  mut pending_token : @agentHost.CancellationToken?
}

fn PollerAgent::new(name : String) -> PollerAgent {
  { name, cancelled: false, pending_token: None }
}

pub fn PollerAgent::poll(self : Self) -> Unit {
  if self.cancelled {
    return
  }
  do_work()
  PollerAgentClient::scoped(self.name, fn(client) raise @common.AgentError {
    let now = @wallClock.now()
    let token = client.schedule_cancelable_poll(@wallClock.Datetime::{
      seconds: now.seconds + 60,
      nanoseconds: 0,
    })
    self.pending_token = Some(token)
  })
}

pub fn PollerAgent::cancel(self : Self) -> Unit {
  self.cancelled = true
  match self.pending_token {
    Some(token) => {
      token.cancel()
      self.pending_token = None
    }
    None => ()
  }
}
```

Note: If you don't cancel the token, call `.drop()` on it to release the resource.

### Cancellation via State Flag

For simpler cases, just use a boolean flag — the next scheduled `poll` checks it and exits early:

```moonbit
pub fn PollerAgent::poll(self : Self) -> Unit {
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
golem agent invocation cancel 'PollerAgent("my-poller")' 'poll-next'
```

## Common Use Cases

### Periodic Polling

Check an external API or queue for new work at regular intervals:

```moonbit
pub fn PollerAgent::poll(self : Self) -> Unit {
  let items = fetch_pending_items()
  items.each(fn(item) { process(item) })
  self.schedule_next(60)
}
```

### Periodic Cleanup

Remove expired data or stale resources on a schedule:

```moonbit
pub fn PollerAgent::cleanup(self : Self) -> Unit {
  self.entries = self.entries.filter(fn(e) { not(e.is_expired()) })
  self.schedule_next(3600) // run hourly
}
```

### Heartbeat / Keep-Alive

Periodically notify an external service that the agent is alive:

```moonbit
pub fn PollerAgent::heartbeat(self : Self) -> Unit {
  send_heartbeat(self.service_url)
  self.schedule_next(30) // every 30s
}
```

## Helper for Scheduling Self

Extract the scheduling logic into a helper to keep methods clean:

```moonbit
fn PollerAgent::schedule_next(self : Self, delay_secs : UInt64) -> Unit {
  PollerAgentClient::scoped(self.name, fn(client) raise @common.AgentError {
    let now = @wallClock.now()
    client.schedule_poll(@wallClock.Datetime::{
      seconds: now.seconds + delay_secs,
      nanoseconds: 0,
    })
  })
}
```

## Key Points

- The agent is durable — if it crashes, the pending scheduled invocation still fires and the agent recovers
- Invocations are sequential — no concurrent executions of `poll` on the same agent
- Each `schedule_` call is a fire-and-forget enqueue; the current invocation completes immediately
- Use a state flag to stop the loop gracefully
- Keep the scheduled method idempotent — it may be retried on recovery
