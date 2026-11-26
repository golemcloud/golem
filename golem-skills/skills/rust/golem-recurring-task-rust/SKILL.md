---
name: golem-recurring-task-rust
description: "Implements recurring Rust agent work by self-scheduling future invocations. Use for periodic jobs, polling loops, heartbeats, cleanup, or retry backoff."
---

# Recurring tasks via self-scheduling (Rust)

A durable agent can schedule its own next invocation after completing each tick. Scheduled
invocations survive recovery and execute sequentially with the agent's other invocations.

Use the current `#[agent_definition]` and `#[agent_implementation]` macros, construct a
`golem_rust::ScheduledTime`, pass method arguments before the scheduled time, and handle the
generated scheduling result:

```rust
use golem_rust::{ScheduledTime, agent_definition, agent_implementation};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn after(delay: Duration) -> ScheduledTime {
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        + delay;
    ScheduledTime {
        seconds: at.as_secs() as i64,
        nanoseconds: at.subsec_nanos(),
    }
}

#[agent_definition]
pub trait PollerAgent {
    fn new(name: String) -> Self;
    async fn poll(&mut self, endpoint: String);
}

struct PollerAgentImpl {
    name: String,
    stopped: bool,
}

#[agent_implementation]
impl PollerAgent for PollerAgentImpl {
    fn new(name: String) -> Self {
        Self { name, stopped: false }
    }

    async fn poll(&mut self, endpoint: String) {
        if self.stopped {
            return;
        }

        do_work(&endpoint).await;

        let me = PollerAgentClient::upsert(self.name.clone());
        me.schedule_poll(endpoint, after(Duration::from_secs(60)))
            .await
            .expect("failed to schedule next poll");
    }
}
```

Generated scheduling methods are async and return
`Result<golem_rust::Invocation<R>, golem_rust::GolemReflectError>` once the target has durably
accepted the invocation. Never discard the error: a failed enqueue breaks the recurring chain.
Dropping the returned handle does not cancel the scheduled invocation. For a method with
arguments, the signature is `schedule_<method>(arguments..., scheduled_time)`; `ScheduledTime` is
always last.

## Backoff

Store the consecutive failure count in agent state. Reset it after success; otherwise compute a
capped delay and schedule one next invocation. For example:

```rust
let delay = if succeeded {
    self.consecutive_failures = 0;
    60
} else {
    self.consecutive_failures += 1;
    (60 * 2u64.pow(self.consecutive_failures.min(6))).min(3600)
};

PollerAgentClient::upsert(self.name.clone())
    .schedule_poll(endpoint, after(Duration::from_secs(delay)))
    .await
    .expect("failed to schedule retry");
```

## Cancel the pending tick

`schedule_<method>` returns the handle of the scheduled invocation. Store its ID, a
`golem_rust::OwnedInvocationId`, in agent state, and rebuild the handle with
`golem_rust::Invocation::<R>::get` when a later invocation must cancel the tick:

```rust
use golem_rust::{Invocation, OwnedInvocationId};

// In `poll`, after scheduling the next tick (`self.pending: Option<OwnedInvocationId>`):
let next = PollerAgentClient::upsert(self.name.clone())
    .schedule_poll(endpoint, after(Duration::from_secs(60)))
    .await
    .expect("failed to schedule next poll");
self.pending = Some(next.id().clone());

// In a `stop` method:
self.stopped = true;
if let Some(id) = self.pending.take() {
    Invocation::<()>::get(&id)
        .cancel()
        .await
        .expect("failed to cancel the pending poll");
}
```

`cancel` returns `true` when it withdrew the tick before it started and `false` when the tick
already finished or was already cancelled. It fails with `GolemReflectError::CannotCancelRunning`
for a running invocation; the agent runs one invocation at a time, so its own pending tick is not
running while `stop` runs. The agent that scheduled the tick may cancel it; other callers need
management permission. Keep the state flag as well: a tick whose ID is not in `self.pending`,
for example one started from the CLI, still exits without doing work. Keep only one pending tick
unless overlapping schedules are intentional.

Recurring invocations grow the oplog. For long-lived or frequent loops, configure periodic
snapshots; do not try to bypass durability. Load `golem-custom-snapshot-rust` for that workflow.
