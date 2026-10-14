// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! One emitter per agent, each holding at most one operation (GOL-370).
//!
//! [`crate::chaos::workload`] drives streams: a shared rate, a shared in-flight
//! budget, and agents picked round-robin out of a pool. That is the right shape
//! when the thing being disturbed is a *stream*, and the wrong one when it is a
//! *place*.
//!
//! S3 cuts one executor off from worker-service. Every agent that executor owns
//! stalls, and those agents are spread evenly through every stream, so a shared
//! budget would be consumed by stalled operations within seconds and the agents
//! on the reachable executor would stop being submitted too. The run would then
//! show the undisturbed half degrading in lockstep with the disturbed one, and
//! the cause would be the driver rather than the platform. The mixed workload
//! already carries a comment about exactly this failure along the stream axis;
//! this module is the same lesson along the ownership axis.
//!
//! So: one task per agent, one operation in flight at a time, and a cadence
//! measured from the end of the previous operation rather than from a shared
//! clock. An agent whose executor is unreachable then contributes nothing and
//! costs nothing, which is what makes the two groups' throughput comparable.

use crate::chaos::history::Stream;
use crate::chaos::workload::{self, WorkloadContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::info;

/// The agents a run of `count` emitters drives, in index order.
///
/// Delegates to [`WorkloadContext::agent_name`] rather than formatting a name
/// of its own, so the two cannot drift. That matters: the shared read-back and
/// exactly-once machinery both select on [`Stream::Durable`] and on names of
/// exactly that shape, and a second copy of the format here would empty both
/// silently rather than fail.
pub fn agent_names(ctx: &WorkloadContext, count: u32) -> Vec<String> {
    (0..count)
        .map(|index| ctx.agent_name(Stream::Durable, index))
        .collect()
}

/// A running steady workload. Dropping the handle does not stop it — call
/// [`SteadyHandle::stop`], so operations in flight record themselves rather than
/// being cancelled. During a partition those are precisely the operations the
/// run exists to describe.
pub struct SteadyHandle {
    stop: Arc<AtomicU8>,
    tasks: JoinSet<()>,
    submitted: Arc<AtomicU64>,
}

impl SteadyHandle {
    pub fn submitted(&self) -> u64 {
        self.submitted.load(Ordering::Relaxed)
    }

    pub async fn stop(mut self) {
        self.stop.store(1, Ordering::Relaxed);
        while self.tasks.join_next().await.is_some() {}
        info!(
            "Chaos steady workload stopped after {} operations",
            self.submitted()
        );
    }
}

/// Starts one emitter per agent and keeps them running until
/// [`SteadyHandle::stop`].
pub fn start(ctx: WorkloadContext, agents: u32, interval: Duration) -> SteadyHandle {
    let stop = Arc::new(AtomicU8::new(0));
    let submitted = Arc::new(AtomicU64::new(0));
    let mut tasks = JoinSet::new();

    info!(
        "Chaos steady workload starting: {agents} emitters, one operation each, {interval:?} \
         between them"
    );

    for index in 0..agents {
        let ctx = ctx.clone();
        let stop = stop.clone();
        let submitted = submitted.clone();

        tasks.spawn(async move {
            let mut seq: u64 = 0;
            while stop.load(Ordering::Relaxed) == 0 {
                submitted.fetch_add(1, Ordering::Relaxed);
                workload::submit_one(&ctx, Stream::Durable, index, seq).await;
                seq += 1;
                // From the end of the operation, not from a fixed clock. An
                // agent that just spent two minutes stalled must not then fire
                // a burst of catch-up operations: that would put its recovery
                // throughput above its baseline and make the recovery cell
                // unreadable.
                if stop.load(Ordering::Relaxed) == 0 {
                    tokio::time::sleep(interval).await;
                }
            }
        });
    }

    SteadyHandle {
        stop,
        tasks,
        submitted,
    }
}
