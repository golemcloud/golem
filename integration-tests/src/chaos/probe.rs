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

//! Asking the platform what it finally did with a key (GOL-366, GOL-364).
//!
//! `Worker::invoke_internal` looks an idempotency key's result up before
//! enqueuing anything, so re-invoking under a key that already ran replays the
//! stored result instead of running again. That turns two questions that are
//! otherwise unanswerable from the client side into direct observations:
//!
//! - *Did accepted work end up with a final result?* The probe returns one, or
//!   it does not.
//! - *Did anything run twice?* A key whose probe returns a **different** value
//!   from the one the driver was given executed a second time.
//!
//! Shared by every scenario with an exactly-once claim to make. S8 uses it on
//! its pinned population; S1 uses it on the durable stream, because "two
//! executors owned one shard" is not directly observable from outside the
//! cluster but *its consequence* — one key executing twice — is.
//!
//! Reading the durable counters immediately before and after the pass bounds
//! the probe's own footprint: the delta is exactly how many keys had never run,
//! which is what makes "replayed a stored result" and "executed fresh work"
//! distinguishable in aggregate.

use crate::chaos::errors::ErrorClass;
use crate::chaos::history::{OperationRecord, Stream};
use crate::chaos::workload::{self, WorkloadContext};
use golem_common::base_model::agent::ParsedAgentId;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::{info, warn};

/// Agent type every probed stream lands on.
const COUNTER_AGENT: &str = "Counter";

/// The method a probe invokes.
///
/// For a key that already ran this is irrelevant — the stored result comes back
/// and the arguments are never looked at. For a key that never ran it executes,
/// and `sleep_and_increment(0)` is exactly `increment`, which is what both the
/// durable and the pinned streams do. One method covers both, and the fresh
/// execution still shows up in the counter delta.
const PROBE_METHOD: &str = "sleep_and_increment";
const PROBE_MILLIS: u32 = 0;

/// How many probes run at once. A probe pass is thousands of replays, so it
/// wants some concurrency — but it runs against a cluster that has just been
/// through a fault, and hammering it would be a second experiment nobody asked
/// for.
const PROBE_CONCURRENCY: usize = 32;

/// Ceiling on a single probe invocation.
///
/// A probe replays a stored result, which is fast — but it runs against agents
/// a fault has just been through, and one of them refusing to answer must cost
/// one key rather than the run. A quota lease lost mid-fault parks the agent's
/// next reservation with no timeout on the platform side, so an unbounded probe
/// against it never returns.
///
/// A timed-out probe is *inconclusive*, never a missing result: the driver could
/// not ask, which says nothing about whether the platform holds the answer.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// How often to report progress through a probe pass.
///
/// A pass covers tens of thousands of keys and logs nothing between its opening
/// line and its verdict, which makes "slow" and "wedged" look identical from a
/// job log. They are not the same and the difference is worth a line every few
/// hundred keys.
const PROBE_PROGRESS_EVERY: usize = 2_000;

/// What the platform said when a key was asked about again after recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyProbe {
    pub idempotency_key: String,
    pub agent: String,
    /// The value the platform returned. `None` means the probe itself failed,
    /// which for an accepted operation is the scenario's hard failure: after
    /// recovery, work the platform took has to have a result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_value: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_class: Option<ErrorClass>,
}

/// Re-invokes every recorded key under its original idempotency key.
///
/// Read the module documentation before changing anything here: the pass only
/// means what it means because the key is reused verbatim and because the
/// counters are read on either side of it.
pub async fn probe_keys(
    ctx: &WorkloadContext,
    records: &[OperationRecord],
    stream: Stream,
) -> Vec<KeyProbe> {
    let keys: Vec<(String, String)> = records
        .iter()
        .filter(|r| r.stream == stream)
        .map(|r| (r.idempotency_key.clone(), r.agent.clone()))
        .collect();

    info!(
        "Chaos: probing {} {stream} keys for their final results",
        keys.len()
    );

    let total = keys.len();
    let mut probes = Vec::with_capacity(total);
    let mut next_report = PROBE_PROGRESS_EVERY;
    for chunk in keys.chunks(PROBE_CONCURRENCY) {
        let mut batch = JoinSet::new();
        for (key, agent) in chunk {
            let ctx = ctx.clone();
            let key = key.clone();
            let agent = agent.clone();
            batch.spawn(async move { probe_one(&ctx, &agent, &key).await });
        }
        while let Some(joined) = batch.join_next().await {
            match joined {
                Ok(probe) => probes.push(probe),
                Err(e) => warn!("Chaos: a probe task panicked: {e}"),
            }
        }
        if probes.len() >= next_report {
            info!("Chaos: probed {} of {total} {stream} keys", probes.len());
            next_report += PROBE_PROGRESS_EVERY;
        }
    }

    probes.sort_by(|a, b| a.idempotency_key.cmp(&b.idempotency_key));
    probes
}

/// Probes one key, retrying once on a transport failure.
///
/// The retry follows the same rule as the workload itself — one attempt, same
/// idempotency key, transport failures only — and here it is purely about not
/// losing a key to a single dropped connection. A probe that cannot complete
/// leaves the key inconclusive, and an inconclusive key weakens the verdict; a
/// cheap same-key retry buys most of them back. It cannot mask a real problem,
/// because a definite refusal is not retried.
async fn probe_one(ctx: &WorkloadContext, agent: &str, key: &str) -> KeyProbe {
    let parsed: ParsedAgentId = agent_id!(COUNTER_AGENT, agent.to_string());
    let idempotency_key = golem_common::model::IdempotencyKey::new(key.to_string());

    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let invoke = ctx.user.invoke_and_await_agent_with_key(
            &ctx.counters,
            &parsed,
            &idempotency_key,
            PROBE_METHOD,
            data_value!(PROBE_MILLIS),
        );
        let outcome = match tokio::time::timeout(PROBE_TIMEOUT, invoke).await {
            Ok(outcome) => outcome,
            // Treated exactly as a transport failure: retried once under the
            // same key, then left inconclusive. An agent that will not answer
            // tells the driver nothing about what the platform holds.
            Err(_) => Err(anyhow::anyhow!("probe timed out after {PROBE_TIMEOUT:?}")),
        };

        match outcome {
            Ok(value) => {
                return KeyProbe {
                    idempotency_key: key.to_string(),
                    agent: agent.to_string(),
                    final_value: workload::as_u32_value(value),
                    error: None,
                    error_class: None,
                };
            }
            Err(e) => {
                let class = crate::chaos::errors::classify(&e);
                if attempts <= ctx.retry.max_retries && class.is_retryable_transport_failure() {
                    tokio::time::sleep(ctx.retry.delay()).await;
                    continue;
                }
                if !class.is_definite_rejection() {
                    warn!(
                        "Chaos: probe of {key} could not complete ({class}), leaving the key \
                         inconclusive rather than reporting a lost result: {e:#}"
                    );
                }
                return KeyProbe {
                    idempotency_key: key.to_string(),
                    agent: agent.to_string(),
                    final_value: None,
                    error: Some(format!("{e:#}")),
                    error_class: Some(class),
                };
            }
        }
    }
}
