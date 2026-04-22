---
name: golem-recurring-task-rust
description: "Implementing a recurring (cron-like) task in a Rust Golem agent by self-scheduling future invocations. Use when the user asks about periodic tasks, recurring jobs, cron-like scheduling, polling loops, heartbeats, or self-scheduling agents."
---

# Recurring Tasks via Self-Scheduling (Rust)

## Overview

A Golem agent can act as its own scheduler by calling `schedule_` on itself at the end of each invocation. This creates a durable, crash-resilient recurring task — if the agent restarts, the scheduled invocation is still pending and will fire at the designated time.

## Basic Pattern

The agent schedules its own method to run again after a delay:

```rust
use golem_rust::wasip2::clocks::wall_clock::Datetime;

#[agent_definition]
pub trait PollerAgent: HasSchema {
    fn new(name: String) -> Self;
    fn start(&mut self);
    fn poll(&mut self);
}

impl PollerAgent for PollerAgentImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    fn start(&mut self) {
        // Kick off the first poll
        self.poll();
    }

    fn poll(&mut self) {
        // 1. Do the recurring work
        do_work();

        // 2. Schedule the next run (60 seconds from now)
        let mut client = PollerAgentClient::get(self.name.clone());
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        client.schedule_poll(Datetime {
            seconds: now_secs + 60,
            nanoseconds: 0,
        });
    }
}
```

## Exponential Backoff

Increase the delay on repeated failures, reset on success:

```rust
fn poll(&mut self) {
    let success = try_work();

    let delay = if success {
        self.consecutive_failures = 0;
        self.base_interval_secs // e.g. 60
    } else {
        self.consecutive_failures += 1;
        let backoff = self.base_interval_secs * 2u64.pow(self.consecutive_failures.min(6));
        backoff.min(self.max_interval_secs) // cap at e.g. 3600
    };

    let mut client = PollerAgentClient::get(self.name.clone());
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    client.schedule_poll(Datetime {
        seconds: now_secs + delay,
        nanoseconds: 0,
    });
}
```

## Cancellation with CancellationToken

The Rust SDK generates `schedule_cancelable_{method}` variants that return a `CancellationToken`. Store the token and cancel it to stop the next scheduled invocation:

```rust
fn poll(&mut self) {
    if self.cancelled {
        return; // stop the loop
    }

    do_work();

    // Schedule next run and store the cancellation token
    let mut client = PollerAgentClient::get(self.name.clone());
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    self.pending_token = Some(client.schedule_cancelable_poll(Datetime {
        seconds: now_secs + 60,
        nanoseconds: 0,
    }));
}

fn cancel(&mut self) {
    self.cancelled = true;
    // Cancel the pending scheduled invocation so it never fires
    if let Some(token) = self.pending_token.take() {
        token.cancel();
    }
}
```

### Cancellation via State Flag

For simpler cases, just use a boolean flag — the next scheduled `poll` checks it and exits early:

```rust
fn poll(&mut self) {
    if self.cancelled {
        return;
    }
    do_work();
    self.schedule_next(60);
}

fn cancel(&mut self) {
    self.cancelled = true;
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

```rust
fn poll(&mut self) {
    let items = fetch_pending_items();
    for item in items {
        process(item);
    }
    self.schedule_next(60); // poll again in 60s
}
```

### Periodic Cleanup

Remove expired data or stale resources on a schedule:

```rust
fn cleanup(&mut self) {
    self.entries.retain(|e| !e.is_expired());
    self.schedule_next(3600); // run hourly
}
```

### Heartbeat / Keep-Alive

Periodically notify an external service that the agent is alive:

```rust
fn heartbeat(&mut self) {
    send_heartbeat(&self.service_url);
    self.schedule_next(30); // every 30s
}
```

## Helper for Scheduling Self

Extract the scheduling logic into a helper to keep methods clean:

```rust
impl PollerAgentImpl {
    fn schedule_next(&self, delay_secs: u64) {
        let mut client = PollerAgentClient::get(self.name.clone());
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        client.schedule_poll(Datetime {
            seconds: now_secs + delay_secs,
            nanoseconds: 0,
        });
    }
}
```

## Key Points

- The agent is durable — if it crashes, the pending scheduled invocation still fires and the agent recovers
- Invocations are sequential — no concurrent executions of `poll` on the same agent
- Each `schedule_` call is a fire-and-forget enqueue; the current invocation completes immediately
- Use a state flag or generation counter to stop the loop gracefully
- Keep the scheduled method idempotent — it may be retried on recovery
