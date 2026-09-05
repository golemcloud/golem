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

//! The scheduled-registration workload (GOL-378).
//!
//! The mixed workload's scheduled stream registers a poll and counts the fires.
//! This one registers a `fire` carrying the registration's own idempotency key,
//! which is what lets [`crate::chaos::fires`] pair an individual action against
//! the invocation that asked for it.
//!
//! ## Why the cadence, and why the lead
//!
//! `lead x targets / interval` is the population that matters: actions the
//! platform has accepted and not yet run. At 100 targets registering every two
//! seconds, ten seconds ahead, that is five hundred of them standing at any
//! instant, spread across both executors' shards. A kill lands in the middle of
//! that population by construction, which is what makes the cadence and the
//! lead the two numbers that decide whether the run measures anything.
//!
//! The lead also has to comfortably exceed the workflow's inject-and-verify
//! path — signal poll, `kubectl apply`, waiting for `AllInjected` — or every
//! action registered before the kill would already have fired by the time the
//! pod died. See [`crate::chaos::scenarios::s10`] for why the narrower
//! claim-to-acknowledge window is not something the driver tries to aim at.
//!
//! ## Why the target set is split rather than pinned
//!
//! [`crate::chaos::pinned`] drives *only* the agents its chosen executor owns,
//! because an S8 operation that was not on the dead pod says nothing. Here the
//! opposite is true: the actions on the surviving executor are the control
//! group. Every target is driven, the driver names the executor owning the
//! largest share, and the report splits the two so a lease recovery that took
//! its full TTL cannot hide behind the half of the population that was never
//! disturbed.

use crate::chaos::ScheduledConfig;
use crate::chaos::history::{FireRecord, Stream, TargetFireLog};
use crate::chaos::workload::{self, SCHEDULE_EMITTER_AGENT, WorkloadContext};
use chrono::{DateTime, Utc};
use golem_common::base_model::agent::ParsedAgentId;
use golem_common::{agent_id, data_value};
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::dsl::TestDsl;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{info, warn};

/// Registrations one target may have in flight at once.
///
/// Small on purpose. The cadence is what sets the rate; this only keeps a
/// stalled platform from accumulating tasks, and a target that has spent its
/// budget skips its tick and says so rather than queueing behind itself.
pub const MAX_IN_FLIGHT_PER_TARGET: usize = 8;

/// How many targets are read back at once. Same reasoning as
/// [`crate::chaos::scenarios::read_back_agents`]: reads do not mutate, and
/// walking them one at a time behind a per-read ceiling outlasts the
/// maintenance window.
const READ_CONCURRENCY: usize = 16;

/// The executor S10 aims at, and how its targets divide around it.
///
/// An alias rather than a type of its own: S11 splits its waiters exactly the
/// same way, the logic lives in [`crate::chaos::split`], and the name is kept
/// here because `scheduledSelection` is the key the archived result and the
/// golem-cloud report both read.
pub type ScheduledSelection = crate::chaos::split::PodSplit;

/// Chooses the executor to aim at. See [`crate::chaos::split::select`].
pub async fn select(
    ctx: &WorkloadContext,
    deps: &BenchmarkTestDependencies,
    targets: &[String],
) -> anyhow::Result<ScheduledSelection> {
    crate::chaos::split::select(crate::chaos::split::schedule_subject(ctx), deps, targets).await
}

/// Re-checks the division immediately before injection. See
/// [`crate::chaos::split::verify_ownership`].
pub async fn verify_ownership(
    ctx: &WorkloadContext,
    deps: &BenchmarkTestDependencies,
    selection: &ScheduledSelection,
) -> anyhow::Result<()> {
    crate::chaos::split::verify_ownership(
        crate::chaos::split::schedule_subject(ctx),
        deps,
        selection,
    )
    .await
}

/// Extra quiet after the last registration's action is due, before the fire
/// logs are read.
///
/// The rest of the settle is derived from the configuration rather than fixed:
/// the final registration falls due one `lead` after the workload stops, and if
/// its target's executor was faulted while holding the claim, the recovery costs
/// up to one lease budget on top. Reading before that elapsed would report
/// actions as lost that were merely late, which is the one mistake a fire
/// account cannot afford.
const SETTLE_MARGIN: Duration = Duration::from_secs(30);

/// How many targets to sample after the baseline to prove actions are firing at
/// all.
///
/// A handful, because this is a smoke test rather than a measurement: if the
/// scheduling path is broken, every target is equally broken, and the point is
/// to fail before spending the fault window on a run that would report a clean
/// account of nothing.
pub const FIRE_PROOF_SAMPLE: usize = 5;

/// How long to wait after the workload stops before reading the fire logs.
pub fn settle_before_readback(config: &ScheduledConfig) -> Duration {
    config.lead() + config.lease_budget() + SETTLE_MARGIN
}

/// Reads the fire count of a few targets, to prove actions are firing at all.
///
/// Registering is not firing. A platform that accepted every registration and
/// scheduled none of them would otherwise reach read-back and report a flawless
/// account of a mechanism that never ran.
pub async fn sample_fire_count(ctx: &WorkloadContext, targets: &[String]) -> u64 {
    let mut total = 0u64;
    for target in targets.iter().take(FIRE_PROOF_SAMPLE) {
        match crate::chaos::workload::read_polls(ctx, target).await {
            Ok(polls) => total += polls,
            Err(e) => warn!("could not sample fires on {target}: {e}"),
        }
    }
    total
}

/// A running registration workload. As elsewhere, dropping the handle does not
/// stop it: call [`ScheduledHandle::stop`] so in-flight registrations record
/// themselves instead of being cancelled mid-flight.
pub struct ScheduledHandle {
    stop: Arc<AtomicU8>,
    tasks: JoinSet<()>,
    submitted: Arc<AtomicU64>,
    skipped: Arc<AtomicU64>,
}

impl ScheduledHandle {
    pub fn submitted(&self) -> u64 {
        self.submitted.load(Ordering::Relaxed)
    }

    /// Ticks a target dropped because it already had its budget of
    /// registrations in flight. Non-zero means the platform was slow enough to
    /// clamp the cadence, which is context for reading everything else.
    pub fn skipped(&self) -> u64 {
        self.skipped.load(Ordering::Relaxed)
    }

    pub async fn stop(mut self) {
        self.stop.store(1, Ordering::Relaxed);
        while self.tasks.join_next().await.is_some() {}
        info!(
            "Chaos scheduled workload stopped after {} registrations ({} ticks skipped)",
            self.submitted(),
            self.skipped()
        );
    }
}

/// Starts one registration loop per target.
pub fn start(
    ctx: WorkloadContext,
    targets: &[String],
    config: &ScheduledConfig,
) -> ScheduledHandle {
    let stop = Arc::new(AtomicU8::new(0));
    let submitted = Arc::new(AtomicU64::new(0));
    let skipped = Arc::new(AtomicU64::new(0));
    let mut tasks = JoinSet::new();
    let interval = config.interval();
    let lead = config.lead();
    let count = targets.len().max(1);

    info!(
        "Chaos scheduled workload starting: {} targets, one registration every {:?} each \
         ({:.1}/s overall), {:?} ahead",
        targets.len(),
        interval,
        targets.len() as f64 / interval.as_secs_f64(),
        lead
    );

    for (index, target) in targets.iter().enumerate() {
        let ctx = ctx.clone();
        let stop = stop.clone();
        let submitted = submitted.clone();
        let skipped = skipped.clone();
        let target = target.clone();
        // Spread the loops across one interval so the whole population does not
        // register in the same instant, which would make the offered rate a
        // sawtooth instead of the constant the phase stats assume.
        let stagger = interval.mul_f64(index as f64 / count as f64);

        tasks.spawn(async move {
            tokio::time::sleep(stagger).await;
            let budget = Arc::new(Semaphore::new(MAX_IN_FLIGHT_PER_TARGET));
            let mut in_flight = JoinSet::new();
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut seq = 0u64;

            while stop.load(Ordering::Relaxed) == 0 {
                ticker.tick().await;
                if stop.load(Ordering::Relaxed) != 0 {
                    break;
                }
                let Ok(permit) = budget.clone().try_acquire_owned() else {
                    skipped.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                submitted.fetch_add(1, Ordering::Relaxed);
                let ctx = ctx.clone();
                let target = target.clone();
                let this_seq = seq;
                seq += 1;
                in_flight.spawn(async move {
                    let _permit = permit;
                    register_one(&ctx, index as u32, &target, this_seq, lead).await;
                });
                while in_flight.try_join_next().is_some() {}
            }

            // Drain rather than cancel: a registration cancelled mid-flight is
            // one the history cannot classify, and during a fault those are the
            // interesting ones.
            while in_flight.join_next().await.is_some() {}
        });
    }

    ScheduledHandle {
        stop,
        tasks,
        submitted,
        skipped,
    }
}

/// Registers one action, `lead` from now, under a key the fire will carry back.
async fn register_one(ctx: &WorkloadContext, index: u32, target: &str, seq: u64, lead: Duration) {
    let emitter = ctx.agent_name(Stream::Scheduled, index);
    let key = ctx.idempotency_key(target, seq);
    let parsed: ParsedAgentId = agent_id!(SCHEDULE_EMITTER_AGENT, emitter);

    // Computed once, outside the retry, so a retried registration asks for the
    // same instant. A due time that moved with each attempt would make the fire
    // log's `scheduledMillis` disagree with what the driver believes it asked
    // for, and the delay measurement would be against the wrong baseline.
    let fire_at = SystemTime::now() + lead;
    let since_epoch = fire_at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let (secs, nanos) = (since_epoch.as_secs(), since_epoch.subsec_nanos());

    let ctx2 = ctx.clone();
    let target2 = target.to_string();
    // Recorded against the target rather than the emitter, because the target is
    // where the fire lands and where the read-back looks.
    workload::run_operation(
        ctx,
        Stream::Scheduled,
        target.to_string(),
        "schedule_fire_at",
        key,
        |k| {
            let ctx = ctx2.clone();
            let parsed = parsed.clone();
            let target = target2.clone();
            async move {
                let token = k.value.clone();
                ctx.user
                    .invoke_and_await_agent_with_key(
                        &ctx.counters,
                        &parsed,
                        &k,
                        "schedule_fire_at",
                        data_value!(target, secs, nanos, token),
                    )
                    .await?;
                Ok(None)
            }
        },
    )
    .await;
}

/// Creates every emitter and target before the baseline starts.
///
/// Returns how many agents were touched. Residency matters here for the same
/// reason it does in S8: an agent that has to cold-start on first use would put
/// start-up cost inside the baseline the recovery is measured against.
pub async fn warm(ctx: &WorkloadContext, targets: &[String]) -> usize {
    let mut warmed = 0usize;
    for (offset, chunk) in targets.chunks(READ_CONCURRENCY).enumerate() {
        let mut batch = JoinSet::new();
        for (position, target) in chunk.iter().cloned().enumerate() {
            let ctx = ctx.clone();
            let index = (offset * READ_CONCURRENCY + position) as u32;
            batch.spawn(async move {
                let emitter = ctx.agent_name(Stream::Scheduled, index);
                let parsed: ParsedAgentId = agent_id!(SCHEDULE_EMITTER_AGENT, emitter.clone());
                if let Err(e) = ctx
                    .user
                    .invoke_and_await_agent(&ctx.counters, &parsed, "warm", data_value!())
                    .await
                {
                    warn!("S10: could not warm emitter {emitter}: {e:#}");
                }
                // Reading the target creates it without mutating it, which is
                // exactly the side effect wanted.
                if let Err(e) = workload::read_polls(&ctx, &target).await {
                    warn!("S10: could not warm target {target}: {e}");
                }
            });
        }
        while batch.join_next().await.is_some() {
            warmed += 1;
        }
    }
    warmed
}

/// Reads every target's fire log back.
///
/// A failed read is carried as [`TargetFireLog::error`] rather than dropped: an
/// agent that could not be read says nothing either way about its actions, and
/// the account has to be able to tell that apart from an agent that lost them.
pub async fn read_logs(ctx: &WorkloadContext, targets: &[String]) -> Vec<TargetFireLog> {
    let total = targets.len();
    let mut logs = Vec::with_capacity(total);

    for chunk in targets.chunks(READ_CONCURRENCY) {
        let mut batch = JoinSet::new();
        for target in chunk.iter().cloned() {
            let ctx = ctx.clone();
            batch.spawn(async move {
                let polls = workload::read_polls(&ctx, &target).await.ok();
                let fires = workload::read_fires(&ctx, &target).await;
                match fires {
                    Ok(raw) => TargetFireLog {
                        agent: target,
                        polls,
                        fires: to_fire_records(&raw),
                        error: None,
                    },
                    Err(e) => TargetFireLog {
                        agent: target,
                        polls,
                        fires: Vec::new(),
                        error: Some(e),
                    },
                }
            });
        }

        let mut batch_results = Vec::new();
        while let Some(joined) = batch.join_next().await {
            match joined {
                Ok(log) => batch_results.push(log),
                Err(e) => warn!("S10: a fire-log read task panicked: {e}"),
            }
        }
        batch_results.sort_by(|a, b| a.agent.cmp(&b.agent));
        logs.extend(batch_results);
        info!("S10: read fire logs for {} of {total} targets", logs.len());
    }

    logs
}

/// Turns the agent's raw triples into timestamps.
///
/// An entry whose millis cannot be a timestamp is dropped rather than clamped.
/// Dropping leaves the log shorter than the target's own `polls`, which is what
/// the account reads as "this target cannot testify" — a clamped nonsense
/// timestamp would instead have produced a confident wrong delay.
pub fn to_fire_records(raw: &[(String, u64, u64)]) -> Vec<FireRecord> {
    let mut out = Vec::with_capacity(raw.len());
    for (token, scheduled_millis, observed_millis) in raw {
        match (
            millis_to_time(*scheduled_millis),
            millis_to_time(*observed_millis),
        ) {
            (Some(scheduled_at), Some(observed_at)) => out.push(FireRecord {
                token: token.clone(),
                scheduled_at,
                observed_at,
            }),
            _ => warn!(
                "S10: dropping fire log entry {token} with unreadable timestamps \
                 ({scheduled_millis}, {observed_millis})"
            ),
        }
    }
    out
}

fn millis_to_time(millis: u64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(i64::try_from(millis).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    /// The happy path of the conversion the whole account is built on.
    #[test]
    fn raw_triples_become_timestamped_fire_records() {
        let raw = vec![("t-0".to_string(), 1_800_000_000_000, 1_800_000_000_450)];
        let records = to_fire_records(&raw);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].token, "t-0");
        assert_eq!(records[0].delay_ms(), 450);
    }

    /// An unreadable entry has to shorten the log rather than become a
    /// confident wrong delay: the shortfall against `polls` is what tells the
    /// account this target cannot testify.
    #[test]
    fn an_entry_with_an_impossible_timestamp_is_dropped_rather_than_clamped() {
        let raw = vec![
            ("good".to_string(), 1_800_000_000_000, 1_800_000_000_010),
            ("bad".to_string(), u64::MAX, 1_800_000_000_010),
        ];
        let records = to_fire_records(&raw);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].token, "good");

        let log = TargetFireLog {
            agent: "target-0".to_string(),
            polls: Some(2),
            fires: records,
            error: None,
        };
        assert!(
            !log.is_complete(),
            "a dropped entry must leave the log short of polls"
        );
    }

    /// The last registration falls due one lead after the workload stops, and a
    /// recovery costs up to a lease on top. Reading before that would report
    /// late actions as lost.
    #[test]
    fn the_settle_covers_a_full_lead_plus_a_full_lease_recovery() {
        let config = ScheduledConfig {
            targets: 100,
            interval_millis: 2000,
            lead_secs: 10,
            lease_budget_secs: 45,
        };
        assert_eq!(
            settle_before_readback(&config),
            Duration::from_secs(10 + 45) + SETTLE_MARGIN
        );
    }
}
