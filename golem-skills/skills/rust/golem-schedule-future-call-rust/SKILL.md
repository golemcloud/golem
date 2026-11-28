---
name: golem-schedule-future-call-rust
description: "Schedules a future agent invocation from Rust. Use for delayed processing, reminders, retries, or timed agent execution."
---

# Schedule a future agent invocation (Rust)

Generated agent clients expose an async `schedule_<method>`. Pass the method arguments first and a
`golem_rust::ScheduledTime` last:

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

let reporter = ReportAgentClient::upsert("daily".to_string());
let report = reporter
    .schedule_generate_report("summary".to_string(), scheduled_time)
    .await
    .expect("failed to schedule report");
```

`ScheduledTime` is an absolute Unix timestamp with signed seconds and nanosecond precision. Do not
use the removed `wasip2::clocks::wall_clock::Datetime` path.

`schedule_<method>` returns `Result<golem_rust::Invocation<R>, golem_rust::GolemReflectError>`,
where `R` is the method's return type, for durable and ephemeral targets alike. It returns once the
target has durably accepted the invocation. Dropping the handle does not cancel the invocation.

## Cancel or await the scheduled invocation

The same handle cancels the invocation before it starts, reads its status, or awaits its result:

```rust
// Cancel before the invocation starts.
let withdrawn: bool = report.cancel().await?;
```

`cancel` returns `true` when it withdrew the invocation, `false` when the invocation already
finished or was already cancelled, and fails with `GolemReflectError::CannotCancelRunning` when it
is running. `report.await_result().await` fails with `GolemReflectError::Cancelled` for a
cancelled invocation.

To cancel from a later invocation of the same agent, store the ID instead of the handle and
rebuild the handle with `golem_rust::Invocation::<R>::get`:

```rust
use golem_rust::{Invocation, OwnedInvocationId};

let id: OwnedInvocationId = report.id().clone(); // keep in agent state
// ...later:
Invocation::<Report>::get(&id).cancel().await?;
```

The ID only locates the invocation; it is not authority. Only the agent that scheduled the
invocation, or a caller with management permission, can observe or cancel it. Scheduled calls
never lend a scope card and reject methods with stream parameters or results.

Scheduling is fire-and-forget: it confirms that the invocation was accepted, not that the future
method succeeded. Keep the target method idempotent when retries or recovery may repeat effects.
