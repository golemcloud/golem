---
name: golem-schedule-future-call-rust
description: "Scheduling a future agent invocation in a Rust Golem project. Use when the user asks about delayed invocations, scheduling calls for later, or timed agent execution."
---

# Scheduling a Future Agent Invocation (Rust)

## Overview

A **scheduled invocation** enqueues a method call on the target agent to be executed at a specific future time. The call returns immediately; the target agent processes it when the scheduled time arrives.

## Usage

Every method on the generated `<AgentName>Client` has a corresponding `schedule_` variant that takes a `Datetime` as the first argument:

```rust
use golem_rust::wasip2::clocks::wall_clock::Datetime;

let mut counter = CounterAgentClient::get("my-counter".to_string());

// Schedule increment to run 60 seconds from now
let now_secs = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_secs();

counter.schedule_increment(Datetime {
    seconds: now_secs + 60,
    nanoseconds: 0,
});

// Schedule with arguments
let reporter = ReportAgentClient::get("daily".to_string());
reporter.schedule_generate_report( 
    "summary".to_string(),
    Datetime { seconds: tomorrow_midnight, nanoseconds: 0 }
);
```

## Datetime Type

The `Datetime` struct represents a point in time as seconds + nanoseconds since the Unix epoch:

```rust
use golem_rust::wasip2::clocks::wall_clock::Datetime;

Datetime {
    seconds: 1700000000,  // Unix timestamp in seconds
    nanoseconds: 0,       // Sub-second precision
}
```

## Use Cases

- **Periodic tasks**: Schedule the next run at the end of each invocation
- **Delayed processing**: Process an order after a cooling-off period
- **Reminders and notifications**: Send a reminder at a specific time
- **Retry with backoff**: Schedule a retry after a delay on failure
