---
name: golem-schedule-future-call-rust
description: "Schedules a future agent invocation from Rust. Use for delayed processing, reminders, retries, or timed agent execution."
---

# Schedule a future agent invocation (Rust)

Generated agent clients expose `schedule_<method>` and `schedule_cancelable_<method>`. Pass the
method arguments first and a `golem_rust::ScheduledTime` last:

```rust
use golem_rust::ScheduledTime;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

let at = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .expect("time went backwards")
    + Duration::from_secs(60);
let scheduled_time = ScheduledTime {
    seconds: at.as_secs() as i64,
    nanoseconds: at.subsec_nanos(),
};

let reporter = ReportAgentClient::get("daily".to_string());
reporter
    .schedule_generate_report("summary".to_string(), scheduled_time)
    .expect("failed to schedule report");
```

`ScheduledTime` is an absolute Unix timestamp with signed seconds and nanosecond precision. Do not
use the removed `wasip2::clocks::wall_clock::Datetime` path.

For durable agents, `schedule_<method>` returns `Result<(), RpcError>`, where `RpcError` is
`golem_rust::golem_agentic::golem::agent::host::RpcError`. The cancelable
variant returns a cancellation token:

```rust
let token = reporter
    .schedule_cancelable_generate_report("summary".to_string(), scheduled_time)
    .expect("failed to schedule report");

// Cancel before the invocation starts.
token.cancel();
```

Scheduling is fire-and-forget: it confirms that the invocation was enqueued, not that the future
method succeeded. Keep the target method idempotent when retries or recovery may repeat effects.
