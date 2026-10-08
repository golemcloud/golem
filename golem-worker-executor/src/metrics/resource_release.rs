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

//! Process-local observations of release facts. None of these handles owns cleanup work.

use prometheus::core::{Collector, Desc};
use prometheus::proto::MetricFamily;
use prometheus::{CounterVec, GaugeVec, HistogramOpts, HistogramVec, Opts, Registry};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

pub(crate) const BUCKETS: &[f64] = &[
    0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0,
];

macro_rules! labels {
    ($name:ident { $($variant:ident => $label:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
        pub(crate) enum $name { $($variant),+ }
        impl $name {
            fn label(self) -> &'static str {
                match self { $(Self::$variant => $label),+ }
            }
        }
    };
}

labels!(Origin {
    AcceptedStop => "accepted_stop", Unload => "unload", FilesystemDelete => "filesystem_delete"
});
labels!(Stage {
    PrimaryExecutionQuiesced => "primary_execution_quiesced",
    PrimaryStoreDrop => "primary_store_drop", MemoryGrantRelease => "memory_grant_release",
    FilesystemDrained => "filesystem_drained", WindowPrepared => "window_prepared",
    FilesystemDeleted => "filesystem_deleted", PermitReleased => "permit_released",
    PermitWaitJoined => "permit_wait_joined", HostOperationExit => "host_operation_exit",
    EndToEnd => "end_to_end"
});
labels!(Cause {
    Deleting => "deleting", ExplicitStop => "explicit_stop", Failure => "failure",
    FilesystemLimit => "filesystem_limit", FilesystemPressure => "filesystem_pressure",
    Idle => "idle", Interrupt => "interrupt", MemoryLimit => "memory_limit",
    MemoryPressure => "memory_pressure", OutOfMemory => "out_of_memory", Panic => "panic",
    Restart => "restart", ShardLost => "shard_lost", Suspend => "suspend", Jump => "jump",
    MonthlyCompute => "monthly_compute", MonthlyMemory => "monthly_memory",
    MonthlyDurableStorage => "monthly_durable_storage",
    MonthlyEphemeralStorage => "monthly_ephemeral_storage", Pending => "pending"
});
labels!(Failure {
    Deadline => "deadline", FilesystemObservation => "filesystem_observation",
    FilesystemDelete => "filesystem_delete", ExclusiveOwnership => "exclusive_ownership",
    MeterFault => "meter_fault", ObserverLost => "observer_lost", MonitorJoin => "monitor_join",
    StopDriver => "stop_driver", Panic => "panic", Other => "other"
});
labels!(HostExit { Returned => "returned", Trapped => "trapped", Dropped => "dropped" });

trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}
struct MonotonicClock(Instant);
impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

#[derive(Clone)]
pub(crate) struct ReleaseMetrics(Arc<Metrics>);
struct Metrics {
    events: std::sync::atomic::AtomicU64,
    clock: Arc<dyn Clock>,
    ledger: Mutex<Ledger>,
    release: HistogramVec,
    host: HistogramVec,
    filesystem: HistogramVec,
    failures: CounterVec,
    unfinished: GaugeVec,
    oldest: GaugeVec,
}
#[derive(Default)]
struct Ledger {
    next: u64,
    scopes: BTreeMap<u64, ScopeRecord>,
    partitions: BTreeSet<(Origin, Stage, Cause, bool)>,
}
struct ScopeRecord {
    intervals: Vec<Interval>,
    sealed: bool,
    standalone: bool,
    receipts: BTreeMap<u64, ReceiptRecord>,
    stages: BTreeSet<Stage>,
    failures: BTreeSet<(Stage, Failure)>,
}
struct Interval {
    start: EventTime,
    origin: Origin,
    cause: Cause,
    ended: bool,
    applicable: BTreeSet<u64>,
    observed: BTreeSet<u64>,
    last_end: Option<EventTime>,
}
struct ReceiptRecord {
    stage: Stage,
    accepted_only: bool,
    endpoint: bool,
    completed: Option<(EventTime, Option<HostExit>)>,
}

pub(crate) type EventTime = (Duration, u64);

/// Scalar attachment identity for one startup/resident owner, never a resource owner.
#[derive(Clone)]
pub(crate) struct ReleaseScope(Arc<ScopeToken>);
struct ScopeToken {
    metrics: ReleaseMetrics,
    id: u64,
    starts_in_flight: std::sync::atomic::AtomicUsize,
}
impl std::fmt::Debug for ReleaseScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ReleaseScope").field(&self.0.id).finish()
    }
}
impl Drop for ScopeToken {
    fn drop(&mut self) {
        let mut ledger = self.metrics.0.ledger.lock().unwrap();
        if let Some(record) = ledger.scopes.get_mut(&self.id) {
            if record.intervals.iter().all(|interval| interval.ended) {
                ledger.scopes.remove(&self.id);
            } else if record
                .failures
                .insert((Stage::EndToEnd, Failure::ObserverLost))
            {
                self.metrics
                    .0
                    .failures
                    .with_label_values(&[Stage::EndToEnd.label(), Failure::ObserverLost.label()])
                    .inc();
            }
        }
    }
}

/// Only a physical owner calls `complete`. Dropping an observation never completes it.
#[derive(Clone, Debug)]
pub(crate) struct ReleaseReceipt {
    scope: ReleaseScope,
    id: u64,
}

static METRICS: LazyLock<ReleaseMetrics> = LazyLock::new(|| {
    ReleaseMetrics::register(
        prometheus::default_registry(),
        Arc::new(MonotonicClock(Instant::now())),
    )
});
impl Default for ReleaseScope {
    fn default() -> Self {
        METRICS.scope()
    }
}
impl ReleaseMetrics {
    fn register(registry: &Registry, clock: Arc<dyn Clock>) -> Self {
        let histogram = |name, help, labels: &[&str]| {
            HistogramVec::new(
                HistogramOpts::new(name, help).buckets(BUCKETS.to_vec()),
                labels,
            )
            .unwrap()
        };
        let metrics = Self(Arc::new(Metrics {
            events: std::sync::atomic::AtomicU64::new(0),
            clock,
            ledger: Mutex::new(Ledger::default()),
            release: histogram(
                "golem_agent_resource_release_seconds",
                "Elapsed seconds from accepted stop or ordinary unload to physical owner facts",
                &["origin", "stage", "cause", "outcome"],
            ),
            host: histogram(
                "golem_agent_stop_host_operation_exit_seconds",
                "Elapsed seconds from accepted stop to actual tracked driver exit",
                &["operation", "outcome"],
            ),
            filesystem: histogram(
                "golem_agent_filesystem_lifecycle_seconds",
                "Elapsed seconds from owning filesystem deletion acceptance through verified deletion, including repair",
                &["operation", "outcome"],
            ),
            failures: CounterVec::new(
                Opts::new(
                    "golem_agent_resource_cleanup_failures_total",
                    "Distinct bounded failure facts per logical cleanup obligation",
                ),
                &["stage", "reason"],
            )
            .unwrap(),
            unfinished: GaugeVec::new(
                Opts::new(
                    "golem_agent_resource_release_unfinished",
                    "Outstanding applicable release receipts; end_to_end counts episodes",
                ),
                &["origin", "stage", "cause", "health"],
            )
            .unwrap(),
            oldest: GaugeVec::new(
                Opts::new(
                    "golem_agent_resource_release_oldest_unfinished_age_seconds",
                    "Oldest process-local unfinished release age in seconds",
                ),
                &["origin", "stage", "cause", "health"],
            )
            .unwrap(),
        }));
        registry.register(Box::new(metrics.clone())).unwrap();
        metrics
    }
    pub(crate) fn scope(&self) -> ReleaseScope {
        let mut ledger = self.0.ledger.lock().unwrap();
        ledger.next += 1;
        let id = ledger.next;
        ledger.scopes.insert(
            id,
            ScopeRecord {
                intervals: Vec::new(),
                sealed: false,
                standalone: true,
                receipts: BTreeMap::new(),
                stages: BTreeSet::new(),
                failures: BTreeSet::new(),
            },
        );
        ReleaseScope(Arc::new(ScopeToken {
            metrics: self.clone(),
            id,
            starts_in_flight: std::sync::atomic::AtomicUsize::new(0),
        }))
    }
}
impl ReleaseScope {
    pub(crate) fn now(&self) -> EventTime {
        (
            self.0.metrics.0.clock.now(),
            self.0
                .metrics
                .0
                .events
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        )
    }

    pub(crate) fn unload(&self, cause: Cause) -> ReleaseReceipt {
        let _start = PendingStart::new(self);
        let at = self.now();
        let completion = self.completion_barrier();
        self.start_at(at, Origin::Unload, cause);
        completion
    }

    pub(crate) fn accepted_join(&self) -> ReleaseReceipt {
        self.receipt_for(Stage::EndToEnd, true, true)
    }

    pub(crate) fn accepted_driver(&self) -> ReleaseReceipt {
        let _start = PendingStart::new(self);
        let at = self.now();
        let receipt = self.receipt_for(Stage::EndToEnd, true, true);
        self.start_at(at, Origin::AcceptedStop, Cause::Pending);
        receipt
    }

    pub(crate) fn start(&self, origin: Origin, cause: Cause) {
        let _start = PendingStart::new(self);
        self.start_at(self.now(), origin, cause);
    }

    fn start_at(&self, start: EventTime, origin: Origin, cause: Cause) {
        let mut ledger = self.0.metrics.0.ledger.lock().unwrap();
        let record = ledger.scopes.get_mut(&self.0.id).unwrap();
        if record
            .intervals
            .iter()
            .any(|interval| interval.origin == origin)
            || (origin == Origin::Unload && !record.intervals.is_empty())
        {
            return;
        }
        record.intervals.push(Interval {
            start,
            origin,
            cause,
            ended: false,
            applicable: record
                .receipts
                .iter()
                .filter_map(|(&id, receipt)| {
                    (!receipt.accepted_only || origin == Origin::AcceptedStop)
                        .then_some(id)
                        .filter(|_| receipt.completed.is_none_or(|(end, _)| end > start))
                })
                .collect(),
            observed: BTreeSet::new(),
            last_end: None,
        });
        self.flush(record);
    }
    fn hide_unfinished(&self) {
        self.0
            .metrics
            .0
            .ledger
            .lock()
            .unwrap()
            .scopes
            .get_mut(&self.0.id)
            .unwrap()
            .standalone = false;
    }
    pub(crate) fn freeze_cause(&self, cause: Cause) {
        let mut ledger = self.0.metrics.0.ledger.lock().unwrap();
        let record = ledger.scopes.get_mut(&self.0.id).unwrap();
        for interval in &mut record.intervals {
            if interval.cause == Cause::Pending {
                interval.cause = cause;
            }
        }
        self.flush(record);
    }
    pub(crate) fn receipt(&self, stage: Stage) -> ReleaseReceipt {
        self.receipt_for(stage, false, true)
    }

    pub(crate) fn completion_barrier(&self) -> ReleaseReceipt {
        self.receipt_for(Stage::EndToEnd, false, false)
    }

    fn receipt_for(&self, stage: Stage, accepted_only: bool, endpoint: bool) -> ReleaseReceipt {
        let mut ledger = self.0.metrics.0.ledger.lock().unwrap();
        ledger.next += 1;
        let id = ledger.next;
        let record = ledger.scopes.get_mut(&self.0.id).unwrap();
        record.stages.insert(stage);
        record.receipts.insert(
            id,
            ReceiptRecord {
                stage,
                accepted_only,
                endpoint,
                completed: None,
            },
        );
        for interval in &mut record.intervals {
            if !interval.ended
                && stage != Stage::HostOperationExit
                && (!accepted_only || interval.origin == Origin::AcceptedStop)
            {
                interval.applicable.insert(id);
            }
        }
        ReleaseReceipt {
            scope: self.clone(),
            id,
        }
    }
    pub(crate) fn has_receipt(&self, stage: Stage) -> bool {
        self.0.metrics.0.ledger.lock().unwrap().scopes[&self.0.id]
            .stages
            .contains(&stage)
    }
    pub(crate) fn fail(&self, stage: Stage, reason: Failure) {
        self.record_failure(stage, reason, true);
    }
    fn record_failure(&self, stage: Stage, reason: Failure, count: bool) {
        let mut ledger = self.0.metrics.0.ledger.lock().unwrap();
        let record = ledger.scopes.get_mut(&self.0.id).unwrap();
        if record.failures.insert((stage, reason)) && count {
            self.0
                .metrics
                .0
                .failures
                .with_label_values(&[stage.label(), reason.label()])
                .inc();
        }
    }
    /// The existing owner has attached every applicable physical obligation.
    pub(crate) fn seal(&self) {
        let mut ledger = self.0.metrics.0.ledger.lock().unwrap();
        let record = ledger.scopes.get_mut(&self.0.id).unwrap();
        record.sealed = true;
        self.flush(record);
    }
    fn flush(&self, record: &mut ScopeRecord) {
        let failed = !record.failures.is_empty();
        for interval in &mut record.intervals {
            interval.applicable.retain(|id| {
                record.receipts[id]
                    .completed
                    .is_none_or(|(end, _)| end > interval.start)
            });
            if interval.cause == Cause::Pending || interval.ended {
                continue;
            }
            for &id in &interval.applicable {
                let receipt = &record.receipts[&id];
                let Some((end, host)) = receipt.completed else {
                    continue;
                };
                if !interval.observed.insert(id) {
                    continue;
                }
                if receipt.endpoint {
                    interval.last_end =
                        Some(interval.last_end.map_or(end, |previous| previous.max(end)));
                }
                let seconds = end.0.saturating_sub(interval.start.0).as_secs_f64();
                if receipt.stage == Stage::HostOperationExit {
                    if interval.origin == Origin::AcceptedStop
                        && let Some(outcome) = host
                    {
                        self.0
                            .metrics
                            .0
                            .host
                            .with_label_values(&["p3_tcp_receive", outcome.label()])
                            .observe(seconds);
                    }
                } else if interval.origin == Origin::FilesystemDelete {
                    self.0
                        .metrics
                        .0
                        .filesystem
                        .with_label_values(&[
                            "delete",
                            if failed {
                                "success_after_failure"
                            } else {
                                "success"
                            },
                        ])
                        .observe(seconds);
                } else if receipt.stage != Stage::EndToEnd {
                    self.0
                        .metrics
                        .0
                        .release
                        .with_label_values(&[
                            interval.origin.label(),
                            receipt.stage.label(),
                            interval.cause.label(),
                            if failed {
                                "released_with_failure"
                            } else {
                                "released"
                            },
                        ])
                        .observe(seconds);
                }
            }
            if record.sealed
                && interval
                    .applicable
                    .iter()
                    .all(|id| record.receipts[id].completed.is_some())
            {
                interval.ended = true;
                if interval.origin != Origin::FilesystemDelete {
                    let end = interval.last_end.unwrap_or(interval.start);
                    self.0
                        .metrics
                        .0
                        .release
                        .with_label_values(&[
                            interval.origin.label(),
                            Stage::EndToEnd.label(),
                            interval.cause.label(),
                            if failed {
                                "released_with_failure"
                            } else {
                                "released"
                            },
                        ])
                        .observe(end.0.saturating_sub(interval.start.0).as_secs_f64());
                }
            }
        }
        self.prune(record);
    }

    fn prune(&self, record: &mut ScopeRecord) {
        if self
            .0
            .starts_in_flight
            .load(std::sync::atomic::Ordering::Acquire)
            != 0
        {
            return;
        }
        let completed: Vec<_> = record
            .receipts
            .iter()
            .filter_map(|(&id, receipt)| {
                (receipt.completed.is_some()
                    && record.intervals.iter().all(|interval| {
                        !interval.applicable.contains(&id)
                            || interval.observed.contains(&id)
                            || interval.ended
                    }))
                .then_some(id)
            })
            .collect();
        for id in completed {
            record.receipts.remove(&id);
            for interval in &mut record.intervals {
                interval.applicable.remove(&id);
                interval.observed.remove(&id);
            }
        }
    }
}
impl ReleaseReceipt {
    pub(crate) fn scope(&self) -> &ReleaseScope {
        &self.scope
    }
    pub(crate) fn belongs_to(&self, scope: &ReleaseScope) -> bool {
        Arc::ptr_eq(&self.scope.0, &scope.0)
    }
    pub(crate) fn complete(&self) {
        self.complete_at(self.scope.now());
    }
    pub(crate) fn complete_all<'a>(receipts: impl IntoIterator<Item = &'a Self>) {
        let events: Vec<_> = receipts
            .into_iter()
            .map(|receipt| (receipt, receipt.scope.now()))
            .collect();
        for (receipt, at) in events {
            receipt.complete_at(at);
        }
    }
    pub(crate) fn complete_at(&self, at: EventTime) {
        self.finish(at, None);
    }
    fn finish(&self, at: EventTime, host: Option<HostExit>) {
        let mut ledger = self.scope.0.metrics.0.ledger.lock().unwrap();
        let record = ledger.scopes.get_mut(&self.scope.0.id).unwrap();
        let Some(receipt) = record.receipts.get_mut(&self.id) else {
            return;
        };
        if receipt.completed.is_none() {
            receipt.completed = Some((at, host));
        }
        self.scope.flush(record);
    }
}

// A start timestamp precedes its ledger lock. Keep completion facts until every such
// start has installed its interval, even when an owner reports completion first.
struct PendingStart<'a>(&'a ReleaseScope);
impl<'a> PendingStart<'a> {
    fn new(scope: &'a ReleaseScope) -> Self {
        scope
            .0
            .starts_in_flight
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self(scope)
    }
}
impl Drop for PendingStart<'_> {
    fn drop(&mut self) {
        self.0
            .0
            .starts_in_flight
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        let mut ledger = self.0.0.metrics.0.ledger.lock().unwrap();
        self.0.prune(ledger.scopes.get_mut(&self.0.0.id).unwrap());
    }
}

/// Marks loss of owner proof, never release, when owned cleanup unwinds or is destroyed.
pub(crate) struct CleanupObservation {
    scope: ReleaseScope,
    stage: Stage,
    returned: bool,
    count: bool,
}
impl CleanupObservation {
    pub(crate) fn new(scope: &ReleaseScope, stage: Stage) -> Self {
        Self {
            scope: scope.clone(),
            stage,
            returned: false,
            count: true,
        }
    }
    pub(crate) fn returned(&mut self) {
        self.returned = true;
    }
}
impl Drop for CleanupObservation {
    fn drop(&mut self) {
        if !self.returned {
            self.scope.record_failure(
                self.stage,
                if std::thread::panicking() {
                    Failure::Panic
                } else {
                    Failure::ObserverLost
                },
                self.count,
            );
        }
    }
}

/// Kept by the filesystem generation across failed adapter deletion and explicit repair.
pub(crate) struct FilesystemRelease {
    local: ReleaseScope,
    deletion: ReleaseReceipt,
    parents: Mutex<Vec<ReleaseReceipt>>,
}
impl Default for FilesystemRelease {
    fn default() -> Self {
        Self::new(ReleaseScope::default())
    }
}
impl FilesystemRelease {
    pub(crate) fn new(local: ReleaseScope) -> Self {
        let deletion = local.receipt(Stage::FilesystemDeleted);
        Self {
            local,
            deletion,
            parents: Mutex::default(),
        }
    }
    pub(crate) fn attach(&self, scope: &ReleaseScope) {
        let mut parents = self.parents.lock().unwrap();
        if !parents.iter().any(|receipt| receipt.belongs_to(scope)) {
            parents.push(scope.receipt(Stage::FilesystemDeleted));
            self.local.hide_unfinished();
        }
    }
    pub(crate) fn observe_cleanup(&self) -> Vec<CleanupObservation> {
        let mut observations = vec![CleanupObservation::new(
            &self.local,
            Stage::FilesystemDeleted,
        )];
        observations.extend(self.parents.lock().unwrap().iter().map(|receipt| {
            CleanupObservation {
                scope: receipt.scope.clone(),
                stage: Stage::FilesystemDeleted,
                returned: false,
                count: false,
            }
        }));
        observations
    }
    pub(crate) fn accept(&self) {
        self.local.start(Origin::FilesystemDelete, Cause::Deleting);
        self.local.seal();
    }
    pub(crate) fn fail(&self, reason: Failure) {
        self.local.fail(Stage::FilesystemDeleted, reason);
        for parent in self.parents.lock().unwrap().iter() {
            parent
                .scope
                .record_failure(Stage::FilesystemDeleted, reason, false);
        }
    }
    pub(crate) fn deleted(&self) {
        let at = self.local.now();
        let parents = self.parents.lock().unwrap();
        let events: Vec<_> = parents
            .iter()
            .map(|parent| {
                let stamp = if Arc::ptr_eq(&self.local.0.metrics.0, &parent.scope.0.metrics.0) {
                    at
                } else {
                    parent.scope.now()
                };
                (parent, stamp)
            })
            .collect();
        self.deletion.complete_at(at);
        for (parent, stamp) in events {
            parent.complete_at(stamp);
        }
    }
}

/// Owned by the entered receive driver, not by its stream or result observer.
pub(crate) struct HostDriverExit {
    receipt: ReleaseReceipt,
    outcome: HostExit,
}
impl HostDriverExit {
    pub(crate) fn entered(scope: &ReleaseScope) -> Self {
        Self {
            receipt: scope.receipt(Stage::HostOperationExit),
            outcome: HostExit::Dropped,
        }
    }
    pub(crate) fn returned(&mut self, trapped: bool) {
        self.outcome = if trapped {
            HostExit::Trapped
        } else {
            HostExit::Returned
        };
    }
}
impl Drop for HostDriverExit {
    fn drop(&mut self) {
        self.receipt
            .finish(self.receipt.scope.now(), Some(self.outcome));
    }
}

impl Collector for ReleaseMetrics {
    fn desc(&self) -> Vec<&Desc> {
        [
            &self.0.release as &dyn Collector,
            &self.0.host,
            &self.0.filesystem,
            &self.0.failures,
            &self.0.unfinished,
            &self.0.oldest,
        ]
        .into_iter()
        .flat_map(Collector::desc)
        .collect()
    }
    fn collect(&self) -> Vec<MetricFamily> {
        let mut ledger = self.0.ledger.lock().unwrap();
        let now = self.0.clock.now();
        let mut values = BTreeMap::<_, (usize, f64)>::new();
        for record in ledger.scopes.values() {
            if !record.standalone {
                continue;
            }
            for interval in &record.intervals {
                let age = now.saturating_sub(interval.start.0).as_secs_f64();
                let failed = !record.failures.is_empty();
                let mut add = |stage, health| {
                    let (count, oldest) = values
                        .entry((interval.origin, stage, interval.cause, health))
                        .or_default();
                    *count += 1;
                    *oldest = oldest.max(age);
                };
                if !interval.ended && interval.origin != Origin::FilesystemDelete {
                    add(Stage::EndToEnd, failed);
                }
                for id in &interval.applicable {
                    let receipt = &record.receipts[id];
                    if receipt.completed.is_none() && receipt.stage != Stage::EndToEnd {
                        add(
                            receipt.stage,
                            record
                                .failures
                                .iter()
                                .any(|(stage, _)| *stage == receipt.stage),
                        );
                    }
                }
            }
        }
        ledger.partitions.extend(values.keys().copied());
        // Retain zero-valued partitions so a completed or reclassified series cannot look stale.
        for &(origin, stage, cause, failed) in &ledger.partitions {
            let labels = [
                origin.label(),
                stage.label(),
                cause.label(),
                if failed { "failed" } else { "pending" },
            ];
            let (count, age) = values
                .get(&(origin, stage, cause, failed))
                .copied()
                .unwrap_or_default();
            self.0
                .unfinished
                .with_label_values(&labels)
                .set(count as f64);
            self.0.oldest.with_label_values(&labels).set(age);
        }
        [
            &self.0.release as &dyn Collector,
            &self.0.host,
            &self.0.filesystem,
            &self.0.failures,
            &self.0.unfinished,
            &self.0.oldest,
        ]
        .into_iter()
        .flat_map(Collector::collect)
        .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use prometheus::{Encoder, TextEncoder};
    use std::sync::atomic::{AtomicU64, Ordering};
    use test_r::test;

    #[derive(Default)]
    struct TestClock(AtomicU64);
    impl Clock for TestClock {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.load(Ordering::Relaxed))
        }
    }
    pub(crate) struct TestMetrics {
        registry: Registry,
        metrics: ReleaseMetrics,
        clock: Arc<TestClock>,
    }
    impl TestMetrics {
        pub(crate) fn new() -> Self {
            let registry = Registry::new();
            let clock = Arc::new(TestClock::default());
            let metrics = ReleaseMetrics::register(&registry, clock.clone());
            Self {
                registry,
                metrics,
                clock,
            }
        }
        pub(crate) fn scope(&self) -> ReleaseScope {
            self.metrics.scope()
        }
        pub(crate) fn advance(&self, millis: u64) {
            self.clock.0.fetch_add(millis, Ordering::Relaxed);
        }
        pub(crate) fn value(&self, family: &str, labels: &[(&str, &str)]) -> f64 {
            self.registry
                .gather()
                .iter()
                .filter(|item| item.name() == family)
                .flat_map(|item| item.get_metric())
                .filter(|metric| {
                    labels.iter().all(|(name, value)| {
                        metric
                            .get_label()
                            .iter()
                            .any(|label| label.name() == *name && label.value() == *value)
                    })
                })
                .map(|metric| {
                    if metric.histogram.is_some() {
                        metric.get_histogram().sample_count() as f64
                    } else if metric.counter.is_some() {
                        metric.get_counter().value()
                    } else {
                        metric.get_gauge().value()
                    }
                })
                .sum()
        }
        pub(crate) fn count(&self, origin: &str, stage: &str, outcome: &str) -> f64 {
            self.value(
                "golem_agent_resource_release_seconds",
                &[("origin", origin), ("stage", stage), ("outcome", outcome)],
            )
        }
        pub(crate) fn pending(&self, origin: &str, stage: &str) -> f64 {
            self.value(
                "golem_agent_resource_release_unfinished",
                &[("origin", origin), ("stage", stage)],
            )
        }
        pub(crate) fn assert_finished(&self, origin: &str, outcome: &str) {
            assert_eq!(
                self.count(origin, "end_to_end", outcome),
                1.0,
                "{}",
                self.export()
            );
            assert_eq!(self.pending(origin, "end_to_end"), 0.0, "{}", self.export());
        }
        pub(crate) fn export(&self) -> String {
            let mut output = Vec::new();
            TextEncoder::new()
                .encode(&self.registry.gather(), &mut output)
                .unwrap();
            String::from_utf8(output).unwrap()
        }
    }

    #[test]
    fn registry_exports_exact_buckets_and_scrape_only_age() {
        let metrics = TestMetrics::new();
        let scope = metrics.scope();
        let receipt = scope.receipt(Stage::PermitReleased);
        scope.start(Origin::AcceptedStop, Cause::Pending);
        scope.seal();
        metrics.advance(1250);
        let pending = metrics.export();
        assert!(pending.contains(
            "health=\"pending\",origin=\"accepted_stop\",stage=\"permit_released\"} 1.25"
        ));
        receipt.complete();
        assert!(
            !metrics
                .export()
                .contains("golem_agent_resource_release_seconds_count")
        );
        scope.freeze_cause(Cause::MonthlyMemory);
        let export = metrics.export();
        for boundary in BUCKETS {
            assert!(
                export.contains(&format!("le=\"{boundary}\"")),
                "missing {boundary}: {export}"
            );
        }
        assert!(export.contains("le=\"+Inf\""));
        assert!(export.contains("golem_agent_resource_release_seconds_sum"));
        assert!(export.contains("stage=\"permit_released\"} 1.25"));
        assert!(!export.contains("le=\"0.025\""));
    }

    #[test]
    fn failed_driver_cannot_complete_an_unjoined_startup_task() {
        let metrics = TestMetrics::new();
        let scope = metrics.scope();
        let driver = scope.accepted_driver();
        let waiting = scope.accepted_join();
        scope.freeze_cause(Cause::Interrupt);
        scope.fail(Stage::EndToEnd, Failure::StopDriver);
        scope.seal();
        driver.complete();
        drop(waiting);
        assert_eq!(metrics.pending("accepted_stop", "end_to_end"), 1.0);
        assert_eq!(
            metrics.count("accepted_stop", "end_to_end", "released_with_failure"),
            0.0
        );
        assert_eq!(
            metrics.count("accepted_stop", "permit_wait_joined", "released"),
            0.0
        );
    }

    #[test]
    fn reporting_barriers_do_not_extend_physical_endpoint_time() {
        let metrics = TestMetrics::new();
        let scope = metrics.scope();
        let permit = scope.receipt(Stage::PermitReleased);
        let reported = scope.unload(Cause::Idle);
        scope.seal();
        metrics.advance(1250);
        permit.complete();
        metrics.advance(10000);
        assert_eq!(metrics.pending("unload", "end_to_end"), 1.0);
        reported.complete();
        metrics.assert_finished("unload", "released");
        assert!(metrics.export().contains(
            "cause=\"idle\",origin=\"unload\",outcome=\"released\",stage=\"end_to_end\"} 1.25"
        ));
    }

    #[test]
    fn completed_unstarted_receipts_are_pruned_without_losing_inflight_starts() {
        let metrics = TestMetrics::new();
        let scope = metrics.scope();
        for _ in 0..1000 {
            scope.receipt(Stage::HostOperationExit).complete();
        }
        assert!(
            metrics.metrics.0.ledger.lock().unwrap().scopes[&scope.0.id]
                .receipts
                .is_empty()
        );
        let receipt = scope.receipt(Stage::PermitReleased);
        let starting = PendingStart::new(&scope);
        let accepted_at = scope.now();
        metrics.advance(1000);
        receipt.complete();
        scope.start_at(accepted_at, Origin::AcceptedStop, Cause::Interrupt);
        drop(starting);
        scope.seal();
        metrics.assert_finished("accepted_stop", "released");
        assert_eq!(
            metrics.count("accepted_stop", "permit_released", "released"),
            1.0
        );
        assert!(
            metrics.metrics.0.ledger.lock().unwrap().scopes[&scope.0.id]
                .receipts
                .is_empty()
        );
    }

    #[test]
    fn completed_before_acceptance_is_absent_even_when_receipt_arrives_late() {
        let metrics = TestMetrics::new();
        let scope = metrics.scope();
        let store = scope.receipt(Stage::PrimaryStoreDrop);
        // Simulate a preemption after the clock read but before ordinal allocation.
        let physical_end = (scope.now().0, u64::MAX);
        metrics.advance(1000);
        let driver = scope.accepted_driver();
        scope.freeze_cause(Cause::Interrupt);
        scope.seal();
        store.complete_at(physical_end);
        assert_eq!(
            metrics.count("accepted_stop", "primary_store_drop", "released"),
            0.0
        );
        assert_eq!(metrics.pending("accepted_stop", "primary_store_drop"), 0.0);
        assert_eq!(metrics.pending("accepted_stop", "end_to_end"), 1.0);
        driver.complete();
        metrics.assert_finished("accepted_stop", "released");
    }

    #[test]
    fn accepted_startup_retains_open_attachments_and_reclaims_completed_metadata() {
        let metrics = TestMetrics::new();
        let scope = metrics.scope();
        let driver = scope.accepted_driver();
        scope.freeze_cause(Cause::MonthlyCompute);
        driver.complete();
        assert_eq!(metrics.pending("accepted_stop", "end_to_end"), 1.0);
        let store = scope.receipt(Stage::PrimaryStoreDrop);
        scope.seal();
        assert_eq!(
            metrics.count("accepted_stop", "end_to_end", "released"),
            0.0
        );
        store.complete();
        metrics.assert_finished("accepted_stop", "released");
        drop((driver, store, scope));
        assert!(metrics.metrics.0.ledger.lock().unwrap().scopes.is_empty());
        drop(metrics.scope());
        assert!(metrics.metrics.0.ledger.lock().unwrap().scopes.is_empty());
        let missing = metrics.scope();
        let proof = missing.receipt(Stage::FilesystemDeleted);
        missing.start(Origin::Unload, Cause::Failure);
        missing.seal();
        drop((proof, missing));
        metrics.advance(1000);
        assert_eq!(metrics.pending("unload", "end_to_end"), 1.0);
        assert_eq!(metrics.count("unload", "end_to_end", "released"), 0.0);
        assert_eq!(
            metrics.value(
                "golem_agent_resource_cleanup_failures_total",
                &[("reason", "observer_lost")]
            ),
            1.0
        );
    }

    #[test]
    fn all_release_families_export_the_dedicated_bucket_inventory() {
        let metrics = TestMetrics::new();
        let scope = metrics.scope();
        let mut host = HostDriverExit::entered(&scope);
        let driver = scope.accepted_driver();
        scope.freeze_cause(Cause::MonthlyMemory);
        scope.seal();
        let filesystem = FilesystemRelease::new(metrics.scope());
        filesystem.accept();
        metrics.advance(1250);
        host.returned(false);
        drop(host);
        driver.complete();
        filesystem.deleted();
        let export = metrics.export();
        for family in [
            "golem_agent_resource_release_seconds",
            "golem_agent_stop_host_operation_exit_seconds",
            "golem_agent_filesystem_lifecycle_seconds",
        ] {
            let lines: Vec<_> = export
                .lines()
                .filter(|line| line.starts_with(&format!("{family}_bucket")))
                .collect();
            assert_eq!(lines.len(), BUCKETS.len() + 1, "{export}");
            for boundary in BUCKETS {
                assert!(
                    lines
                        .iter()
                        .any(|line| line.contains(&format!("le=\"{boundary}\""))),
                    "{export}"
                );
            }
            assert!(lines.iter().any(|line| line.contains("le=\"+Inf\"")));
            assert!(export.contains(&format!("{family}_sum")));
            assert!(export.contains(&format!("{family}_count")));
        }
        println!("{export}");
    }

    #[test]
    fn failures_receipts_and_followers_do_not_rewrite_observations() {
        let metrics = TestMetrics::new();
        let scope = metrics.scope();
        let receipt = scope.receipt(Stage::FilesystemDeleted);
        scope.start(Origin::Unload, Cause::Deleting);
        metrics.advance(300_001);
        scope.fail(Stage::FilesystemDeleted, Failure::FilesystemDelete);
        scope.fail(Stage::FilesystemDeleted, Failure::FilesystemDelete);
        scope.seal();
        scope.start(Origin::AcceptedStop, Cause::Interrupt);
        let failed = metrics.export();
        assert!(failed.contains("reason=\"filesystem_delete\",stage=\"filesystem_deleted\"} 1"));
        assert!(!failed.contains("golem_agent_resource_release_seconds_count"));
        drop(receipt.clone());
        assert_eq!(failed, metrics.export());
        receipt.complete();
        let done = metrics.export();
        receipt.complete();
        scope.seal();
        assert_eq!(done, metrics.export());
        assert!(done.contains("outcome=\"released_with_failure\""));
        assert!(!done.contains("outcome=\"released\""));
        assert!(
            !TestMetrics::new()
                .export()
                .contains("released_with_failure")
        );
    }
}
