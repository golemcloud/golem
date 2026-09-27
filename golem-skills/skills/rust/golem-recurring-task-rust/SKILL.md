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

        let me = PollerAgentClient::get(self.name.clone());
        me.schedule_poll(endpoint, after(Duration::from_secs(60)))
            .expect("failed to schedule next poll");
    }
}
```

Generated durable-agent scheduling methods return
`Result<(), golem_rust::golem_agentic::golem::agent::host::RpcError>`. Never discard
that result: a failed enqueue breaks the recurring chain. For a method with arguments, the
signature is `schedule_<method>(arguments..., scheduled_time)`; `ScheduledTime` is always last.

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

PollerAgentClient::get(self.name.clone())
    .schedule_poll(endpoint, after(Duration::from_secs(delay)))
    .expect("failed to schedule retry");
```

## Cancel the pending tick

`schedule_cancelable_<method>(arguments..., scheduled_time)` returns
`Result<CancellationToken, RpcError>` for a durable agent. Store the token in agent state
and call `cancel()` to prevent that scheduled invocation from starting:

```rust
let token = PollerAgentClient::get(self.name.clone())
    .schedule_cancelable_poll(endpoint, after(Duration::from_secs(60)))
    .expect("failed to schedule next poll");
self.pending = Some(token);

if let Some(token) = self.pending.take() {
    token.cancel();
}
```

A state flag is still useful because a tick may already have started when cancellation races with
delivery. Keep only one pending tick unless overlapping schedules are intentional.

Recurring invocations grow the oplog. For long-lived or frequent loops, configure periodic
snapshots; do not try to bypass durability. Load `golem-custom-snapshot-rust` for that workflow.
