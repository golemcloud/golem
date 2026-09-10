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

//! Finds oplog layers holding entries for agents that have gone quiet, and runs one archive step
//! against each.
//!
//! This is the work list, not the mover. `MultiLayerOplog::try_archive_blocking` and
//! `EphemeralOplog::try_archive_blocking` already move a prefix down one layer and report whether a
//! layer below still holds entries. `ScheduledAction::ArchiveOplog` answers the same question from
//! a row written on the oplog commit path, one synchronous scheduler-storage write per invocation
//! held under `update_state_lock`; the sweep answers it from a paginated scan of the layer itself.
//!
//! The scanned layer is self-cleaning: an archive step ends in `drop_prefix`, which removes the
//! key. The scan therefore enumerates work rather than agents.
//!
//! # Failure
//!
//! A non-transient indexed-storage error inside a tick panics through `retry_storage_op`, and this
//! workspace builds with `panic = "abort"`, so it takes the process down. That is how every other
//! oplog operation already behaves; the sweep adds another caller, not another failure mode. There
//! is nothing to catch.
//!
//! # Memory
//!
//! Moving a prefix reads the whole source layer into one `Vec`, in
//! `EphemeralOplog::background_transfer` and again in `BackgroundTransfer::run`. The sweep calls
//! whichever applies rather than adding a third copy, so an agent costs it exactly what
//! `archive_ephemeral_oplog` already costs on teardown. What the sweep adds is a ceiling on how
//! many run at once: the teardown drain spawns one task per finishing invocation with nothing
//! capping it, a tick holds at most `max_concurrency`.
//!
//! That ceiling covers the transfer, not everything an archive leaves behind. Archiving an agent
//! builds a suspended `Worker` through `open_oplog`, and nothing evicts it: `stop_if_evictable`
//! matches only a running instance. Each archived agent therefore keeps an `ActiveWorkers` entry
//! for the life of the pod, outside `max_tracked_agents`. It is inherited from
//! `ScheduledAction::ArchiveOplog`, which built the same worker far more often, and it is why
//! `archive_agent` drains an agent fully rather than leaving a hop for a later tick: from the next
//! tick on, the residency probe reports that agent as running and skips it.

use std::collections::{HashMap, HashSet};
use std::fmt::{self, Display};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::stream::{self, StreamExt};
use golem_common::model::agent::AgentMode;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{AgentId, OwnedAgentId, ShardAssignment, ShardId};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, info, info_span, warn};
use uuid::Uuid;

use crate::metrics::oplog::{record_oplog_sweep_outcome, record_oplog_sweep_tick};
use crate::services::component::ComponentService;
use crate::services::golem_config::OplogSweepConfig;
use crate::services::oplog::{EphemeralOplog, MultiLayerOplog, OplogArchiveService};
use crate::services::scheduler::SchedulerWorkerAccess;
use crate::services::shard::ShardService;
use crate::storage::indexed::{
    IndexedStorage, IndexedStorageLabelledApi, IndexedStorageMetaNamespace,
};
use crate::storage::indexed::{ScanResume, agent_mode_prefix};

/// One archive step: entries for agents of `agent_mode` move out of the layer at `source_level`
/// into the layer below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct RouteId {
    agent_mode: AgentMode,
    source_level: usize,
}

impl Display for RouteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The same spelling the storage layer gives a mode, so a route label and a namespace key
        // name the mode identically.
        write!(
            f,
            "{}-l{}",
            agent_mode_prefix(self.agent_mode),
            self.source_level
        )
    }
}

/// What the sweep decided about one scanned key. A key the tick reached produces exactly one,
/// which is what makes [`tally`] a total fold over what it reached. A shutdown during the archive
/// phase is the one case that leaves scanned keys with no outcome, so `scanned` counts what the
/// tick decided rather than what it walked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The key did not parse as `{component_id}:{agent_name}`.
    Unparseable,
    /// Another executor owns this agent's shard.
    NotOwned,
    /// The layer holds nothing; the key lost its entries between the scan and the probe.
    Empty,
    /// The layer's last index moved since the previous tick, so something is still writing.
    Moving,
    /// This executor is running the agent right now.
    Resident,
    /// The agent's component could not be resolved, so the environment its entries belong to is
    /// unknown.
    Unaddressable,
    /// The archive step never ran: the oplog would not open, or nothing recognised it as
    /// something that can be archived.
    ArchiveFailed,
    /// The agent's entries were moved out of every layer that could pass them on.
    ///
    /// Always a full drain. [`OplogSweeper::archive_agent`] keeps stepping until `try_archive`
    /// reports nothing left below, because `open_oplog` leaves a suspended worker behind that makes
    /// the next tick skip the agent as resident; a partial move would strand the remainder. An
    /// agent the step bound cuts short is reported as [`Outcome::ArchiveFailed`] instead.
    Archived,
}

/// Whether an agent has been quiet long enough to archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Remember this index and reconsider next tick.
    Wait,
    /// The index has not moved since the previous tick.
    Move,
}

/// The agent modes the sweep covers. Fixed on purpose: neither entry is a setting.
///
/// Ephemeral is not optional. While the sweep is enabled
/// `Worker::schedule_oplog_archive_if_needed` returns early for ephemeral agents, so nothing
/// registers `ScheduledAction::ArchiveOplog` for them and this sweep is the only thing left that
/// moves an ephemeral oplog a crashed pod stranded. Dropping the mode from this list would strand
/// those oplogs outright, because that guard reads `OplogSweepConfig::enabled` and knows nothing
/// about this list.
///
/// Durable is absent rather than off, because adding it would not be safe today.
/// `IndexedStorage::append` is a plain `INSERT` against a table with
/// `PRIMARY KEY (namespace, key, id)`, a unique violation is not classified transient, and
/// `retry_storage_op` turns that into a panic under `panic = "abort"`. Re-appending a prefix into
/// an indexed layer therefore takes the pod down, and an archive step that is interrupted between
/// its append and its `drop_prefix` leaves exactly that prefix to be re-appended. Blob targets are
/// safe, since their append is a `put` at a path keyed by the chunk's last index. With the default
/// stack the ephemeral hop targets blob and the durable primary hop targets the indexed compressed
/// layer.
///
/// Extending this needs that append to become an upsert first, and a layer passed to
/// [`OplogSweeper::over_layers`] that can enumerate the primary's keys. Both are code changes, and
/// keeping the list here rather than in `OplogSweepConfig` is what makes them so.
pub const SWEPT_MODES: &[AgentMode] = &[AgentMode::Ephemeral];

/// Slack over the layer count when bounding the archive steps one agent gets in a single tick.
///
/// One step moves a whole layer and reports whether another below it still holds entries
/// (`MultiLayerOplog::archive`), so a correct stack needs at most one step per layer. Bounding at
/// the layer count plus this leaves the guard doing only what it was meant to do -- stop a layer
/// that miscounts `more` from spinning -- rather than also capping a stack that is merely deep.
///
/// A fixed bound did both, and the second was silent: it dropped the agent's tracking entry and
/// reported it archived while layers below still held entries, and `open_oplog` had by then left a
/// suspended worker that makes every later tick skip the agent as resident. The untouched layers
/// stayed where they were for the life of the pod.
const ARCHIVE_STEP_SLACK: usize = 2;

/// Resolved environments held at once. Far above the number of components one executor's shards
/// can hold, so in practice the cache never fills; it is a bound rather than a policy.
const MAX_CACHED_ENVIRONMENTS: usize = 10_000;

/// The shortest a tick interval is allowed to be. A misconfigured zero would otherwise spin.
const MIN_INTERVAL: Duration = Duration::from_millis(100);

/// The shortest a tick deadline is allowed to be. A misconfigured zero would cut every tick at its
/// first boundary, which is not a disabled sweep but a sweep that runs and archives nothing.
const MIN_TICK_DURATION: Duration = Duration::from_secs(1);

/// How many intervals to wait before the next tick, given how the last one ended.
///
/// Doubling rather than stepping, because the thing being waited out is a store under load and
/// the cost of waiting too long is only latency on work that is already deferred by a day. The
/// reset is deliberately to `1` and not to the previous value: one tick that finished inside its
/// deadline is the store saying it has capacity again, and the sweep should take it rather than
/// walk back down.
///
/// Pure so the decision is testable without running the loop; the deadline that produces
/// `over_deadline` is enforced by cancelling the tick's token, which is the same path shutdown
/// already uses and is covered by its own tests.
fn next_backoff(current: u32, over_deadline: bool, cap: u32) -> u32 {
    if over_deadline {
        current.saturating_mul(2).min(cap.max(1))
    } else {
        1
    }
}

/// What a read-only probe concluded about one agent.
enum Decision {
    /// Nothing left to do, and the outcome is final.
    Settled(Outcome),
    /// The agent is quiet, so its entries should move once the scan has finished.
    Archive(AgentId),
}

/// Per-route counters.
///
/// `scanned` counts the keys this route took off the namespace, and every other counter except
/// `truncated` counts an outcome. They no longer sum: an outcome can belong to a candidate an
/// earlier tick scanned, because a tick that runs out of time carries its candidates forward, and
/// [`tally_carried`] deliberately keeps that outcome out of `scanned` so the walk is charged once.
/// A tick cut short during the archive phase leaves a scanned key with no outcome at all, which
/// moves the totals the other way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RouteReport {
    scanned: u64,
    unparseable: u64,
    not_owned: u64,
    empty: u64,
    moving: u64,
    resident: u64,
    unaddressable: u64,
    archive_failed: u64,
    archived: u64,
    /// A budget stopped the tick, a scan failed, or the node began shutting down, before the
    /// namespace was exhausted.
    truncated: bool,
}

impl RouteReport {
    fn record(&self, route: &str, elapsed: std::time::Duration) {
        for (outcome, count) in [
            ("unparseable", self.unparseable),
            ("not_owned", self.not_owned),
            ("empty", self.empty),
            ("moving", self.moving),
            ("resident", self.resident),
            ("unaddressable", self.unaddressable),
            ("archive_failed", self.archive_failed),
            ("archived", self.archived),
        ] {
            record_oplog_sweep_outcome(route, outcome, count);
        }
        record_oplog_sweep_tick(route, elapsed, self.truncated);
    }
}

/// What one call to [`OplogSweeper::sweep_once`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SweepReport {
    routes: Vec<(RouteId, RouteReport)>,
    /// True when the sweep ran with no usable shard assignment, in which case it did nothing.
    unassigned: bool,
}

impl SweepReport {
    #[cfg(test)]
    fn route(&self, id: RouteId) -> RouteReport {
        self.routes
            .iter()
            .find(|(route, _)| *route == id)
            .map(|(_, report)| report.clone())
            .unwrap_or_default()
    }

    fn archived(&self) -> u64 {
        self.routes.iter().map(|(_, r)| r.archived).sum()
    }

    fn scanned(&self) -> u64 {
        self.routes.iter().map(|(_, r)| r.scanned).sum()
    }
}

/// Layer keys are `{component_id}:{agent_name}`, written by `AgentId::to_redis_key`. An agent name
/// may itself contain `:`, so only the first separator is significant.
fn parse_agent_id(key: &str) -> Option<AgentId> {
    let (component, name) = key.split_once(':')?;
    if name.is_empty() {
        return None;
    }
    Some(AgentId {
        component_id: ComponentId(Uuid::parse_str(component).ok()?),
        agent_id: name.to_string(),
    })
}

/// Whether this executor owns the agent under the assignment it is currently running.
fn owns(assignment: &ShardAssignment, agent_id: &AgentId) -> bool {
    let shard_id = ShardId::from_routing_hash(
        ShardId::hash_agent_id(agent_id),
        assignment.number_of_shards,
    );
    assignment.shard_ids.contains(&shard_id)
}

/// The quiet gate. `ScheduledAction::ArchiveOplog` carries the index the agent had when the action
/// was registered and acts only if the current index still matches; with no row to carry an index,
/// the sweep compares against what it saw on its own previous pass.
///
/// The sighting has to come from an *earlier* pass, not merely from an earlier probe. A backend may
/// hand the same key back twice inside one walk -- Redis `SCAN` guarantees a key present throughout
/// is returned at least once, not exactly once -- and comparing indices alone let the second
/// sighting satisfy a gate that the first had just opened. That archives an agent within one pass
/// instead of across two, which skips the interval the gate exists to impose and can race a
/// previous shard owner still writing. The `dedup` before the archive phase cannot catch it: by
/// then both sightings have been assessed, and the two may not even be in the same tick. A pass
/// visits each key once, so an earlier pass is what "quiet since last time" means.
fn assess(remembered: Option<Seen>, current: OplogIndex, pass: u64) -> Verdict {
    match remembered {
        Some(seen) if seen.index == current && seen.pass < pass => Verdict::Move,
        _ => Verdict::Wait,
    }
}

/// Adds two reports. `truncated` is sticky: a tick that stopped early on any page stopped early.
fn merge(left: RouteReport, right: RouteReport) -> RouteReport {
    RouteReport {
        scanned: left.scanned + right.scanned,
        unparseable: left.unparseable + right.unparseable,
        not_owned: left.not_owned + right.not_owned,
        empty: left.empty + right.empty,
        moving: left.moving + right.moving,
        resident: left.resident + right.resident,
        unaddressable: left.unaddressable + right.unaddressable,
        archive_failed: left.archive_failed + right.archive_failed,
        archived: left.archived + right.archived,
        truncated: left.truncated || right.truncated,
    }
}

/// Folds per-key outcomes into a report.
fn tally(outcomes: impl IntoIterator<Item = Outcome>) -> RouteReport {
    outcomes
        .into_iter()
        .fold(RouteReport::default(), |mut report, outcome| {
            report.scanned += 1;
            match outcome {
                Outcome::Unparseable => report.unparseable += 1,
                Outcome::NotOwned => report.not_owned += 1,
                Outcome::Empty => report.empty += 1,
                Outcome::Moving => report.moving += 1,
                Outcome::Resident => report.resident += 1,
                Outcome::Unaddressable => report.unaddressable += 1,
                Outcome::ArchiveFailed => report.archive_failed += 1,
                Outcome::Archived => report.archived += 1,
            }
            report
        })
}

/// How much of a batch actually reached the store. Only these two open the oplog and move
/// entries; every other outcome is decided before that and costs the archive budget nothing.
fn archive_work(outcomes: &[Outcome]) -> u64 {
    outcomes
        .iter()
        .filter(|outcome| matches!(outcome, Outcome::Archived | Outcome::ArchiveFailed))
        .count() as u64
}

/// The same, for outcomes belonging to candidates an earlier tick scanned.
///
/// `scanned` counts what a tick took off the namespace, and a carried candidate was taken off it
/// by the tick that probed it. Counting it again would charge this tick's scan budget for a key it
/// never walked, and on a small budget that is enough to stop the routes behind it.
fn tally_carried(outcomes: impl IntoIterator<Item = Outcome>) -> RouteReport {
    RouteReport {
        scanned: 0,
        ..tally(outcomes)
    }
}

/// Splits a scanned page into the keys worth a storage probe and the outcomes of those that are
/// not, using only the assignment. No I/O has happened yet, so these two filters are free.
fn triage(keys: &[String], assignment: &ShardAssignment) -> (Vec<AgentId>, Vec<Outcome>) {
    let mut candidates = Vec::with_capacity(keys.len());
    let mut settled = Vec::new();
    for key in keys {
        match parse_agent_id(key) {
            None => {
                warn!(key = %key, "Oplog sweep skipping an unparseable layer key");
                settled.push(Outcome::Unparseable);
            }
            Some(agent_id) if !owns(assignment, &agent_id) => settled.push(Outcome::NotOwned),
            Some(agent_id) => candidates.push(agent_id),
        }
    }
    (candidates, settled)
}

/// A memo entry: the index an agent showed, and the scan pass that last saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seen {
    index: OplogIndex,
    pass: u64,
}

/// What a tick has left to spend, shared across its routes.
///
/// `max_scanned_per_tick` and `max_archives_per_tick` are documented as tick budgets and used to be
/// handed to every route in full, so a stack with several compressed source levels multiplied the
/// store work one tick could do by the number of routes.
///
/// A route takes an equal share of what is left rather than the whole remainder, so a busy route
/// cannot starve the ones behind it. Sharing the remainder rather than a fixed slice is what keeps
/// that from wasting budget: a route that finds nothing to do leaves its share to the routes after
/// it. The order routes run in is load-bearing -- lowest source level first, so a tick never hands
/// entries to a layer it is about to drain -- which is why the shares move and the order does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TickBudget {
    scans: u64,
    archives: u64,
    routes_left: u64,
}

impl TickBudget {
    fn new(config: &OplogSweepConfig, routes: usize) -> Self {
        Self {
            scans: config.max_scanned_per_tick.max(1) as u64,
            archives: config.max_archives_per_tick.max(1) as u64,
            routes_left: routes.max(1) as u64,
        }
    }

    /// The next route's share: what is left, split over the routes still to run, rounded up.
    ///
    /// `None` once either budget is spent, which is the point the tick stops. This used to floor
    /// at one of each so that no route was handed a budget it could not take a step with, and the
    /// floor applied to an exhausted budget too: with `max_scanned_per_tick = 1` and four routes a
    /// tick scanned four keys, and a route whose archive phase overshot its page did not stop the
    /// routes behind it. The routes a tick cannot afford are left to the next one, which starts
    /// with them rather than behind the same earlier routes again.
    fn share(&self) -> Option<(u64, u64)> {
        if self.scans == 0 || self.archives == 0 {
            return None;
        }
        let left = self.routes_left.max(1);
        Some((self.scans.div_ceil(left), self.archives.div_ceil(left)))
    }

    /// Books what a route used and drops it from the split.
    fn spend(&mut self, scanned: u64, archived: u64) {
        self.scans = self.scans.saturating_sub(scanned);
        self.archives = self.archives.saturating_sub(archived);
        self.routes_left = self.routes_left.saturating_sub(1);
    }
}

struct Route {
    id: RouteId,
    namespace: IndexedStorageMetaNamespace,
    source: Arc<dyn OplogArchiveService>,
}

pub struct OplogSweeper {
    config: OplogSweepConfig,
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    routes: Vec<Route>,
    shards: Arc<dyn ShardService>,
    components: Arc<dyn ComponentService>,
    worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync>,
    /// How many archive steps one agent gets before [`Self::archive_agent`] gives up on it.
    /// Derived from the stack the sweeper was built over, not configured: see
    /// [`ARCHIVE_STEP_SLACK`].
    max_archive_steps: u32,
    /// The index each agent showed on the previous tick, per route. Losing it costs one extra tick
    /// of latency, never a stranded oplog: the work list comes from storage.
    ///
    /// Ephemeral agent ids are unbounded (an invocation with no phantom id gets a fresh
    /// `Uuid::new_v4()`), and an agent drained by `archive_ephemeral_oplog` leaves the layer for
    /// good, so its entry here would never be visited again. Entries are stamped with the scan
    /// pass that touched them and dropped when a pass completes without seeing them.
    memo: Mutex<HashMap<(RouteId, AgentId), Seen>>,
    /// Where each route's scan stopped, so a budgeted tick resumes rather than restarting. Absent
    /// means start from the beginning of the namespace.
    cursors: Mutex<HashMap<RouteId, ScanResume>>,
    /// Which scan pass each route is on. Bumped when a pass reaches the end of the namespace.
    passes: Mutex<HashMap<RouteId, u64>>,
    /// Candidates a tick probed but ran out of time to archive, per route.
    ///
    /// Probing is the expensive half: one storage read per scanned key that is neither resident
    /// nor someone else's, and against a degraded store a single page of those can outlast the
    /// whole tick deadline. Discarding them meant the deadline stopped the sweep archiving
    /// anything at all, because the scan cursor still advanced and the next pass paid the same
    /// probes and was cut in the same place. A tick spends this list before it scans, so one that
    /// keeps timing out still makes progress.
    ///
    /// Capped at one tick's archive budget. An agent dropped off the end is found again by a later
    /// scan, so forgetting one costs a later visit rather than a stranded oplog.
    deferred: Mutex<HashMap<RouteId, Vec<AgentId>>>,
    /// Environments already resolved, by component.
    ///
    /// A cold lookup costs a registry call, and for a deleted component two of them, once per
    /// component per tick. Repaying that every tick is what let a tick that keeps losing its
    /// deadline inside the resolution loop write back the same carried list forever: the loop
    /// stops on the token, so the tick had archived nothing and learned nothing, and the next one
    /// started from the same place. Remembering means a tick that resolves even one component
    /// makes permanent progress, so the carried list drains in bounded time however slow the
    /// registry is.
    ///
    /// An entry cannot go stale: a component does not move between environments, which is the
    /// same fact that lets `environment_of` fall back to revision zero. Cleared wholesale when it
    /// outgrows [`MAX_CACHED_ENVIRONMENTS`] rather than evicted one at a time, because it is a
    /// cache and losing it costs lookups rather than correctness.
    environments: Mutex<HashMap<ComponentId, EnvironmentId>>,
    /// The route the next tick starts at.
    ///
    /// A tick that runs out of budget, or out of time, leaves the routes it never reached here so
    /// the next one begins with them. Only the starting point moves: the routes a tick does run
    /// still run in stack order, deepest source first, so a tick never hands entries to a layer it
    /// is about to drain.
    route_cursor: Mutex<usize>,
}

impl OplogSweeper {
    /// Derives its routes from the layer stack `lib.rs` already built.
    ///
    /// A layer is a source when it can enumerate its own keys and something sits below it to
    /// receive them. Blob-backed archives answer `None` to `scan_namespace`, so the bottom of the
    /// stack is a target only.
    ///
    /// `archives` is the archive stack alone, so the primary layer is never a source and the
    /// level-0 hop stays with `ScheduledAction::ArchiveOplog`. A mode added to [`SWEPT_MODES`]
    /// would reach the compressed levels and leave that first hop where it is; driving it as well
    /// would mean passing a layer here that implements `OplogArchiveService`.
    ///
    /// Pure: no I/O, no task, no runtime needed. Call [`run`](Self::run) to start ticking.
    pub fn over_layers(
        config: OplogSweepConfig,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        archives: &[Arc<dyn OplogArchiveService>],
        shards: Arc<dyn ShardService>,
        components: Arc<dyn ComponentService>,
        worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync>,
    ) -> Arc<Self> {
        // The bottom layer receives entries and has nowhere to pass them on to.
        let sources = archives.len().saturating_sub(1);
        let mut routes: Vec<Route> = Vec::new();
        for source in archives.iter().take(sources) {
            for agent_mode in SWEPT_MODES {
                let Some(namespace) = source.scan_namespace(*agent_mode) else {
                    continue;
                };
                let source_level = match namespace {
                    IndexedStorageMetaNamespace::CompressedOplog { level, .. } => level,
                    // No archive service answers with this today: the primary layer is not one,
                    // and it is the only layer that would report level 0.
                    IndexedStorageMetaNamespace::Oplog { .. } => 0,
                };
                routes.push(Route {
                    id: RouteId {
                        agent_mode: *agent_mode,
                        source_level,
                    },
                    namespace,
                    source: source.clone(),
                });
            }
        }
        // Lowest source first, so a tick never hands entries to a layer it is about to drain.
        routes.sort_by_key(|route| std::cmp::Reverse(route.id.source_level));

        info!(
            routes = %routes.iter().map(|r| r.id.to_string()).collect::<Vec<_>>().join(","),
            enabled = config.enabled,
            "Oplog sweeper built"
        );

        Arc::new(Self {
            config,
            indexed_storage,
            routes,
            shards,
            components,
            worker_access,
            max_archive_steps: archives.len().saturating_add(ARCHIVE_STEP_SLACK) as u32,
            memo: Mutex::new(HashMap::new()),
            cursors: Mutex::new(HashMap::new()),
            passes: Mutex::new(HashMap::new()),
            deferred: Mutex::new(HashMap::new()),
            environments: Mutex::new(HashMap::new()),
            route_cursor: Mutex::new(0),
        })
    }

    /// Runs ticks until `shutdown` is cancelled, then returns.
    ///
    /// Cancellation is observed between routes, between scan pages, before each component the
    /// archive phase has to resolve, and before each agent it reaches, but never inside one, so
    /// shutting down never interrupts an archive step between its append to the layer below and
    /// its drop from the layer above. An agent already under way is finished, all of its layers,
    /// before the loop returns. Spawn this into the executor's `JoinSet` so that shutdown waits
    /// for the step in flight; a tick stops at the next boundary rather than running to
    /// completion.
    ///
    /// A tick is bounded in time as well as in work, and the loop backs off while that bound keeps
    /// being hit. The two budgets are not interchangeable: `max_scanned_per_tick` and
    /// `max_archives_per_tick` say how much a tick does, `max_tick_duration` says how long it may
    /// hold the executor's shared indexed-storage concurrency doing it. See
    /// `OplogSweepConfig::max_tick_duration` for why those are not the same quantity under a
    /// degraded store, and what measured it.
    ///
    /// Returns immediately on either of two conditions, which differ in what is left behind.
    /// Disabled by config leaves `ScheduledAction::ArchiveOplog` as the only archiving mechanism,
    /// and `Worker::schedule_oplog_archive_if_needed` reads the same flag so that the ephemeral
    /// registration comes back with it. A layer stack that offers no route leaves nothing, and
    /// needs nothing: `CreateOplogConstructor` only builds the lower layers an archive step moves
    /// between when the stack has them, so with none configured neither mechanism has anything to
    /// move.
    pub async fn run(self: Arc<Self>, shutdown: CancellationToken) {
        if !self.config.enabled || self.routes.is_empty() {
            return;
        }
        // Floored, because a zero interval would spin the loop rather than disable it. `enabled`
        // is how the sweep is turned off.
        let interval = self.config.interval.max(MIN_INTERVAL);
        // Floored for the same reason, though the failure it prevents is quieter: a zero deadline
        // would cancel every tick before it reached an agent, so the sweep would keep ticking and
        // keep archiving nothing.
        let tick_deadline = self.config.max_tick_duration.max(MIN_TICK_DURATION);
        // Intervals to wait before the next tick. Doubles while ticks keep hitting their deadline
        // and returns to one the moment a tick finishes inside it. See
        // `OplogSweepConfig::max_backoff_intervals` for why the sweep has to derive this from its
        // own behaviour rather than being told.
        let mut backoff: u32 = 1;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tokio::time::sleep(interval.saturating_mul(backoff)) => {}
            }

            // A child of the shutdown token, so a tick still stops on shutdown, plus a deadline of
            // its own. Cancelling it is how the deadline is enforced: every boundary in
            // `sweep_once` that already tests the token becomes a place the tick can stop for time
            // as well. That is the whole reason the deadline is expressed this way rather than
            // threaded through the loops -- an archive step is never cut between its append below
            // and its drop above, which is the invariant shutdown is written around and this
            // inherits for free.
            //
            // The boundaries inside a route mark that route truncated. The one between routes does
            // not: it drops the routes not yet reached from the report entirely, so once the stack
            // has more than one ephemeral source a deadline can end a tick that reports nothing
            // truncated. Harmless to the oplog, misleading in the metric.
            let tick = shutdown.child_token();
            let sweep = self.sweep_once(&tick);
            let deadline = tokio::time::sleep(tick_deadline);
            tokio::pin!(sweep, deadline);
            let report = loop {
                tokio::select! {
                    // Biased, so a sweep that completed in the same poll the deadline elapsed is
                    // read as completed. Random choice would let an on-time tick take the deadline
                    // arm, and `over_deadline` below would then double the wait after it.
                    biased;
                    report = &mut sweep => break report,
                    // Guarded, because a completed `Sleep` polls ready forever and would otherwise
                    // spin this select once the deadline has passed.
                    _ = &mut deadline, if !tick.is_cancelled() => {
                        tick.cancel();
                    }
                }
            };

            // Shutdown cancels the child too, and a tick cut short by shutdown says nothing about
            // the store, so it must not feed the backoff. The loop is about to exit anyway.
            let over_deadline = tick.is_cancelled() && !shutdown.is_cancelled();
            if over_deadline {
                backoff = next_backoff(backoff, true, self.config.max_backoff_intervals);
                debug!(
                    scanned = report.scanned(),
                    archived = report.archived(),
                    backoff_intervals = backoff,
                    "Oplog sweep tick hit its deadline; backing off"
                );
            } else {
                backoff = next_backoff(backoff, false, self.config.max_backoff_intervals);
            }

            if report.unassigned {
                debug!("Oplog sweep tick skipped: this executor holds no shard assignment");
            } else if report.archived() > 0 {
                debug!(
                    archived = report.archived(),
                    scanned = report.scanned(),
                    "Oplog sweep tick"
                );
            }
        }
        debug!("Oplog sweep loop stopped");
    }

    /// Runs one tick over every route. Never fails: a storage error ends the affected route's tick
    /// and the next one retries, because the work list is the layer itself. A tick is usually a
    /// fraction of a scan pass, which is what `passes` and [`Self::finish_pass`] track.
    ///
    /// The token is the tick's own rather than the executor's: cancelling it stops the tick at its
    /// next boundary, which is how both shutdown and `max_tick_duration` are enforced.
    async fn sweep_once(&self, cancel: &CancellationToken) -> SweepReport {
        // Without an assignment every agent would look like someone else's, and archiving an
        // agent this executor may not own is what the shard check is for. A zero-shard assignment
        // is the same state in a different shape: `ShardService` installs
        // `ShardAssignment::default` before it has a shard count to record, and routing an agent
        // through that count divides by zero.
        let assignment = self.shards.try_get_current_assignment();
        let Some(assignment) = assignment.filter(|it| it.number_of_shards > 0) else {
            return SweepReport {
                routes: Vec::new(),
                unassigned: true,
            };
        };

        // Where the last tick ran out of budget or time. `self.routes` is built once in
        // `over_layers` and never changes, so the clamp cannot fire; it costs one comparison and
        // saves the next reader having to prove that.
        let start = {
            let cursor = *self.route_cursor.lock().await;
            if cursor < self.routes.len() {
                cursor
            } else {
                0
            }
        };
        let remaining = self.routes.len() - start;
        let mut routes = Vec::with_capacity(remaining);
        let mut budget = TickBudget::new(&self.config, remaining);
        let mut stopped_at = None;
        for (offset, route) in self.routes[start..].iter().enumerate() {
            if cancel.is_cancelled() {
                stopped_at = Some(start + offset);
                break;
            }
            let Some((scans, archives)) = budget.share() else {
                stopped_at = Some(start + offset);
                break;
            };
            let report = self
                .sweep_route(route, &assignment, cancel, scans, archives)
                .instrument(info_span!("oplog_sweep", route = %route.id))
                .await;
            // `scanned` counts the keys this route took off the namespace. Candidates carried
            // from an earlier tick are not among them: that tick already paid for the walk, and
            // charging this one again would spend a scan budget on keys nobody walked.
            budget.spend(report.scanned, report.archived);
            routes.push((route.id, report));
        }
        // A tick that reached the end of the stack starts the next one at the top. One that
        // stopped early leaves the routes it never reached to go first, so a short budget cannot
        // serve the same prefix of the stack forever.
        *self.route_cursor.lock().await = stopped_at.unwrap_or(0);

        SweepReport {
            routes,
            unassigned: false,
        }
    }

    /// `scan_budget` and `archive_budget` are this route's share of the tick's budgets, not the
    /// configured values: see [`TickBudget`].
    async fn sweep_route(
        &self,
        route: &Route,
        assignment: &ShardAssignment,
        cancel: &CancellationToken,
        scan_budget: u64,
        archive_budget: u64,
    ) -> RouteReport {
        let started = Instant::now();
        let mut resume = self.cursors.lock().await.get(&route.id).cloned();
        // Folded page by page: holding every outcome would make a tick's memory grow with the
        // namespace, which is what the budgets exist to prevent.
        let mut report = RouteReport::default();
        let mut truncated = false;
        let mut exhausted = false;
        let pass = self
            .passes
            .lock()
            .await
            .get(&route.id)
            .copied()
            .unwrap_or(0);

        // What the last tick probed and ran out of time to archive, spent before the scan below
        // gets the chance to spend this tick's deadline the same way. Without this a store slow
        // enough to cut every tick during probing produced a sweep that advanced its cursor
        // forever and archived nothing, which is the one condition the deadline exists for. See
        // [`OplogSweeper::deferred`].
        let mut owed: Vec<AgentId> = Vec::new();
        // Agents the carried batch reached. An agent that survived it, because its component would
        // not resolve or its archive step failed, keeps its key in the layer and can be walked
        // again by the scan below: the previous tick finishing its pass clears the cursor and
        // bumps the pass, and `assess` then opens the gate on the sighting the carried attempt
        // restamped. Archiving it twice in one tick would double-count it and charge the budgets
        // twice for one agent's work.
        let mut handled: HashSet<AgentId> = HashSet::new();
        let mut archive_allowance = archive_budget;
        let mut carried = self.take_deferred(route.id).await;
        if !carried.is_empty() {
            // The scanned path drops what this executor no longer owns in `triage`, before the key
            // ever reaches a probe. A carried candidate was triaged against the assignment of the
            // tick that found it, and a reshard since then would move it to another executor, so
            // it is checked again here rather than archived on an answer that may no longer hold.
            let held = carried.len();
            carried.retain(|agent_id| owns(assignment, agent_id));
            report = merge(
                report,
                tally_carried(std::iter::repeat_n(Outcome::NotOwned, held - carried.len())),
            );

            owed.extend(carried.split_off((archive_allowance as usize).min(carried.len())));
            handled.extend(carried.iter().cloned());
            let (outcomes, declined) = self.archive_batch(route, carried, cancel).await;
            // Charged for the archive steps rather than for the length of the list. A carried
            // agent that turns out to be resident, or whose layer emptied under it, costs the
            // store nothing, and charging for it would let a list of those exhaust the allowance
            // and stop the scan below from running at all.
            archive_allowance = archive_allowance.saturating_sub(archive_work(&outcomes));
            owed.extend(declined);
            report = merge(report, tally_carried(outcomes));
        }

        // Nothing is archived while the scan is paging. `scan_stable` resumes by seeking rather
        // than by counting, so deleting a key behind the walk cannot shift the rest of it, but a
        // tick that archived while paging would still hand the archive phase a list built from a
        // namespace that changed underneath it. Reading first keeps the two apart. The carried
        // batch above is not an exception: it finishes before the walk starts, and a walk that
        // resumes by seeking does not care what left the namespace before it began.
        let mut pending: Vec<AgentId> = Vec::new();
        let mut walked: u64 = 0;
        let mut pages: u64 = 0;
        // Zero would read nothing, exhaust the namespace on the first page and wipe the tracking
        // table, all while reporting a healthy tick, so treat it as one.
        let page_size = self.config.page_size.max(1);
        let scan_budget = scan_budget.max(1);
        // A key budget bounds round trips only on a backend that fills every page. Redis matches
        // server-side and can hand back empty pages while it walks the keyspace, so `walked` would
        // never move and the tick would traverse the whole thing. Bound the pages as well.
        let page_budget = scan_budget.div_ceil(page_size).max(1);
        loop {
            if cancel.is_cancelled() {
                truncated = true;
                break;
            }
            // Finding more than the tick can archive only lengthens the list the next one
            // inherits, and the carried work is older than anything a scan would turn up.
            if archive_allowance == 0 {
                truncated = true;
                break;
            }
            let allowance = scan_budget.saturating_sub(walked);
            if allowance == 0 || pages >= page_budget {
                truncated = true;
                break;
            }
            pages += 1;
            // Asking for only what the budget allows is what keeps `max_scanned_per_tick` close.
            // A backend may still hand back more than it was asked for, but trimming a page after
            // the fact would carry the walk past keys nothing examined.
            let count = page_size.min(allowance);

            let page = self
                .indexed_storage
                .with("oplog_sweep", "scan")
                .scan_stable(route.namespace.clone(), None, resume.clone(), count)
                .await;

            let (next, keys) = match page {
                Ok(page) => page,
                Err(error) => {
                    warn!(route = %route.id, "Oplog sweep scan failed: {error}");
                    truncated = true;
                    break;
                }
            };
            walked += keys.len() as u64;

            let (candidates, settled) = triage(&keys, assignment);
            let probed: Vec<Decision> = stream::iter(candidates)
                .map(|agent_id| self.probe_agent(route, agent_id, pass))
                .buffer_unordered(self.config.max_concurrency.max(1))
                .collect()
                .await;
            let mut outcomes = settled;
            for decision in probed {
                match decision {
                    Decision::Settled(outcome) => outcomes.push(outcome),
                    Decision::Archive(agent_id) => pending.push(agent_id),
                }
            }
            report = merge(report, tally(outcomes));

            resume = next;
            if resume.is_none() {
                exhausted = true;
                break;
            }
            // Soft by up to a page: a page is probed as a unit, so this stops at the first page
            // that carries the budget past its bound rather than splitting one.
            if pending.len() as u64 >= archive_allowance {
                truncated = true;
                break;
            }
        }

        pending.retain(|agent_id| !handled.contains(agent_id));
        let (outcomes, declined) = self.archive_batch(route, pending, cancel).await;
        owed.extend(declined);
        report = merge(report, tally(outcomes));

        // Anything still owed means the tick stopped before its work list did.
        if !owed.is_empty() {
            truncated = true;
        }
        self.defer(route.id, owed).await;

        // Stored exactly as it came back. The archive phase above deleted keys the walk had
        // already passed, and with a positional cursor that would have shifted every key behind
        // them and left this tick a correction it could only estimate. A resume token names a place
        // in the key order, so nothing the archive phase did can move it.
        match resume {
            Some(resume) => {
                self.cursors.lock().await.insert(route.id, resume);
            }
            None => {
                self.cursors.lock().await.remove(&route.id);
            }
        }

        if exhausted {
            // A pass covered the whole namespace, so anything it did not see has left the layer,
            // usually drained by `archive_ephemeral_oplog`. Those agents never come back under the
            // same id, so their entries are dropped here rather than left to fill the table.
            self.finish_pass(route.id, pass).await;
        }
        self.forget_stale(route.id, assignment).await;

        report.truncated = truncated;
        report.record(&route.id.to_string(), started.elapsed());
        report
    }

    /// Resolves environments and archives, and hands back the agents it never reached.
    ///
    /// A cancelled tick declines the rest rather than dropping them. The probe that put an agent
    /// on this list is a storage read, and throwing that away is what let a tick that keeps
    /// hitting its deadline archive nothing: see [`OplogSweeper::deferred`].
    async fn archive_batch(
        &self,
        route: &Route,
        mut agents: Vec<AgentId>,
        cancel: &CancellationToken,
    ) -> (Vec<Outcome>, Vec<AgentId>) {
        // A backend is allowed to hand the same key back twice in one walk, and archiving an agent
        // twice would race two transfers and count one of them twice.
        agents.sort();
        agents.dedup();

        // The layer below addresses its objects by environment and a scanned key does not carry
        // one, so each agent needs its component resolved. `get_metadata` hands back the whole
        // component, and ephemeral agents come in crowds that share one, so resolve each component
        // once rather than once per agent.
        let components: HashSet<ComponentId> = agents
            .iter()
            .map(|agent_id| agent_id.component_id)
            .collect();
        let mut environments: HashMap<ComponentId, Option<EnvironmentId>> =
            HashMap::with_capacity(components.len());
        for component_id in components {
            // Same boundary as the archiving below. A cold cache makes each of these a registry
            // call, and there is one per component the list turned up, so a cancelled tick should
            // not have to wait them out. Leaving the rest unresolved costs nothing: every agent
            // below tests the token before it reads this map.
            if cancel.is_cancelled() {
                break;
            }
            let resolved = self.environment_of(component_id).await;
            environments.insert(component_id, resolved);
        }

        // Now the deletes. One stream over the whole list rather than fixed batches, because the
        // cost of an agent varies with how much its layers hold and a batch would run at the speed
        // of its slowest member. The cancellation check sits before an agent and never inside one,
        // so a cancelled tick finishes the agents under way and starts no more.
        let results: Vec<Result<Outcome, AgentId>> = stream::iter(agents)
            .map(|agent_id| {
                let environment_id = environments.get(&agent_id.component_id).copied().flatten();
                async move {
                    if cancel.is_cancelled() {
                        return Err(agent_id);
                    }
                    // Asked again on both paths, not only the carried one. The residency answer
                    // that put an agent on this list was taken while the scan was still paging,
                    // which on a slow store is seconds before the archive phase reaches it, and
                    // for a carried candidate it is a whole interval older than that. An agent
                    // that started running in either window is reported as resident instead of
                    // being archived under a live writer.
                    if self.is_resident(&agent_id, environment_id).await {
                        return Ok(Outcome::Resident);
                    }
                    Ok(self.archive_agent(route, agent_id, environment_id).await)
                }
            })
            .buffer_unordered(self.config.max_concurrency.max(1))
            .collect()
            .await;

        let mut outcomes = Vec::with_capacity(results.len());
        let mut declined = Vec::new();
        for result in results {
            match result {
                Ok(outcome) => outcomes.push(outcome),
                Err(agent_id) => declined.push(agent_id),
            }
        }
        (outcomes, declined)
    }

    /// Takes the candidates the last tick could not reach, leaving the route owing nothing.
    async fn take_deferred(&self, route: RouteId) -> Vec<AgentId> {
        self.deferred
            .lock()
            .await
            .remove(&route)
            .unwrap_or_default()
    }

    /// Records what this tick could not reach, capped at one tick's archive budget.
    ///
    /// A route that keeps being cut short cannot grow this without bound. An agent dropped from
    /// the tail is found again by a later scan, because the work list is the layer itself, so the
    /// cap costs a later visit rather than a stranded oplog.
    async fn defer(&self, route: RouteId, mut agents: Vec<AgentId>) {
        if agents.is_empty() {
            self.deferred.lock().await.remove(&route);
            return;
        }
        agents.sort();
        agents.dedup();
        agents.truncate(self.config.max_archives_per_tick.max(1));
        self.deferred.lock().await.insert(route, agents);
    }

    /// Whether this executor is running the agent right now.
    ///
    /// In memory, so it costs nothing to ask. `ActiveWorkers` is keyed by agent and mode alone, so
    /// any environment addresses the same entry and a caller that has not resolved one yet can
    /// still ask.
    async fn is_resident(&self, agent_id: &AgentId, environment_id: Option<EnvironmentId>) -> bool {
        let probe = OwnedAgentId {
            environment_id: environment_id.unwrap_or_default(),
            agent_id: agent_id.clone(),
        };
        self.worker_access
            .active_worker_fingerprint(&probe)
            .await
            .is_some()
    }

    /// Decides one agent without touching it.
    ///
    /// Reads only. A whole page of these can run while the scan is still walking the namespace,
    /// because nothing here removes a key.
    ///
    /// The checks run cheapest first. Both the residency probe and the index probe are keyed by
    /// agent and mode alone, so they need no environment.
    async fn probe_agent(&self, route: &Route, agent_id: AgentId, pass: u64) -> Decision {
        // Reading an indexed layer, and looking an agent up in `ActiveWorkers`, are both keyed by
        // agent and mode only, so any environment addresses the same thing. The real one is
        // resolved in `archive_agent`, once it is needed.
        let probe = OwnedAgentId {
            environment_id: EnvironmentId::new(),
            agent_id: agent_id.clone(),
        };

        // In memory, and it comes first because on a busy executor most scanned keys belong to
        // agents this pod is running. Probing their index would be one storage read each, every
        // tick, to learn what this answers for free.
        if self.is_resident(&agent_id, None).await {
            return Decision::Settled(Outcome::Resident);
        }

        let current = route
            .source
            .get_last_index(&probe, route.id.agent_mode)
            .await;
        if current == OplogIndex::NONE {
            self.forget(route.id, &agent_id).await;
            return Decision::Settled(Outcome::Empty);
        }

        let remembered = self
            .memo
            .lock()
            .await
            .get(&(route.id, agent_id.clone()))
            .copied();
        if assess(remembered, current, pass) == Verdict::Wait {
            self.remember(route.id, &agent_id, current, pass).await;
            return Decision::Settled(Outcome::Moving);
        }

        // Restamped even though it is about to be archived. A successful archive calls `forget` and
        // the entry goes anyway, but an archive that cannot resolve the component, cannot open the
        // oplog, or never runs because the node is shutting down leaves the agent in the layer. On
        // the old stamp `finish_pass` would drop it, and the agent would start its two-tick gate
        // again on every pass without ever getting through it.
        self.remember(route.id, &agent_id, current, pass).await;
        Decision::Archive(agent_id)
    }

    /// Moves one agent's entries out of this layer, and keeps going until no layer below holds
    /// any, or until [`OplogSweeper::max_archive_steps`] steps have run.
    ///
    /// Runs only after the scan has finished paging, because every step ends in a `drop_prefix`
    /// that removes the key the scan walked.
    async fn archive_agent(
        &self,
        route: &Route,
        agent_id: AgentId,
        environment_id: Option<EnvironmentId>,
    ) -> Outcome {
        let Some(environment_id) = environment_id else {
            return Outcome::Unaddressable;
        };
        let owned_agent_id = OwnedAgentId {
            environment_id,
            agent_id: agent_id.clone(),
        };

        // Building the suspended `Worker` is the mutual exclusion, exactly as it is for
        // `ScheduledAction::ArchiveOplog`, which reaches this same call once its own gates pass.
        let oplog = match self.worker_access.open_oplog(&owned_agent_id).await {
            Ok(oplog) => oplog,
            Err(error) => {
                warn!(
                    agent_id = %agent_id,
                    "Oplog sweep could not open the oplog for archiving: {error}"
                );
                return Outcome::ArchiveFailed;
            }
        };

        // One step moves a whole layer and reports whether another below it still holds entries,
        // so this ends after at most one step per layer.
        //
        // It has to finish the agent here rather than leave the rest to a later tick. `open_oplog`
        // registers a suspended worker that nothing evicts, so from the next tick on the residency
        // probe reports this agent as running and skips it, and a half-moved tail would stay in
        // the layer for the life of the pod.
        let mut more = true;
        let mut steps = 0;
        while more && steps < self.max_archive_steps {
            let stepped = match MultiLayerOplog::try_archive_blocking(&oplog).await {
                Some(more) => Some(more),
                None => EphemeralOplog::try_archive_blocking(&oplog).await,
            };
            match stepped {
                Some(remaining) => more = remaining,
                // Neither archive step recognised this oplog, so nothing moved. Reporting it as
                // archived would drop the tracking entry and leave the key where it is, and the
                // agent would qualify again every two ticks for as long as the pod lives.
                None => {
                    warn!(
                        agent_id = %agent_id,
                        "Oplog sweep found a layer it cannot archive"
                    );
                    return Outcome::ArchiveFailed;
                }
            }
            steps += 1;
        }
        if more {
            // Not an archive, and the tracking entry stays. Reporting this as archived would drop
            // the entry while layers below still hold entries, and the suspended worker
            // `open_oplog` just registered makes the next tick skip the agent as resident, so the
            // remainder would never be reached again. The bound is the stack depth plus slack, so
            // reaching it means a layer reported more work than the stack can hold.
            warn!(
                agent_id = %agent_id,
                steps,
                "Oplog sweep stopped archiving an agent at the step bound; a layer reported more \
                 work below it than the stack has layers"
            );
            return Outcome::ArchiveFailed;
        }

        self.forget(route.id, &agent_id).await;
        debug!(agent_id = %agent_id, steps, "Oplog sweep archived an agent");
        // If a durable route is ever added to `SWEPT_MODES`, this is where
        // `WorkerService::remove_cached_status` belongs, as it does in the scheduled action.
        Outcome::Archived
    }

    /// An agent's environment, resolved through its component.
    ///
    /// The layer below addresses its objects by environment and a scanned key does not carry one.
    /// The `Create` entry does, but it is the oplog's first entry and an earlier archive step will
    /// have moved it out of the layer being scanned, which is where any agent that outgrew
    /// `entry_count_limit` ends up. The component answers instead, because an agent's environment
    /// is its component's environment.
    ///
    /// Two lookups, because the deployed one alone strands exactly the oplogs this sweep exists to
    /// reach. `get_metadata(id, None)` goes to `get_deployed_component_metadata`, which answers only
    /// for a component that is currently deployed and not deleted. An agent whose teardown archival
    /// was lost to a pod crash therefore becomes permanently unarchiveable the moment its component
    /// is deleted: every later tick resolves nothing, reports [`Outcome::Unaddressable`], and leaves
    /// the entries where they are. `ScheduledAction::ArchiveOplog` never had that problem, because
    /// its row carried the `OwnedAgentId`.
    ///
    /// The first revision answers instead. `get_component_metadata` is documented to return deleted
    /// components, and a component does not move between environments across revisions, so revision
    /// zero carries the same environment the current one would. Deployed is still tried first: it is
    /// the lookup the rest of the executor warms a cache for, and most swept agents belong to
    /// components that are still there.
    async fn environment_of(&self, component_id: ComponentId) -> Option<EnvironmentId> {
        if let Some(environment_id) = self.environments.lock().await.get(&component_id).copied() {
            return Some(environment_id);
        }
        let resolved = if let Ok(component) = self.components.get_metadata(component_id, None).await
        {
            Some(component.environment_id)
        } else {
            match self
                .components
                .get_metadata(component_id, Some(ComponentRevision::INITIAL))
                .await
            {
                Ok(component) => Some(component.environment_id),
                Err(error) => {
                    warn!(
                        component_id = %component_id,
                        "Oplog sweep could not resolve the component of a stranded oplog: {error}"
                    );
                    None
                }
            }
        };
        if let Some(environment_id) = resolved {
            let mut cache = self.environments.lock().await;
            if cache.len() >= MAX_CACHED_ENVIRONMENTS {
                cache.clear();
            }
            cache.insert(component_id, environment_id);
        }
        resolved
    }

    async fn remember(&self, route: RouteId, agent_id: &AgentId, index: OplogIndex, pass: u64) {
        let mut memo = self.memo.lock().await;
        let key = (route, agent_id.clone());
        if memo.len() >= self.config.max_tracked_agents.max(1) && !memo.contains_key(&key) {
            // A backstop, not the mechanism: `finish_pass` drops entries whose agents have left the
            // layer, so reaching this means a single pass is tracking more agents than the bound
            // allows.
            //
            // Declining this one agent costs it a pass. Clearing the table instead would cost every
            // tracked agent the sighting it already has, and since an agent needs two sightings at
            // the same index to qualify, a pass that refills the table would wipe it again at the
            // same point and the sweep would archive nothing at all.
            warn!(
                tracked = memo.len(),
                agent_id = %agent_id,
                "Oplog sweep tracking table full, not tracking this agent"
            );
            return;
        }
        // An agent already tracked still updates, or a moving oplog could never reach a second
        // sighting once the table filled.
        memo.insert(key, Seen { index, pass });
    }

    /// Closes a scan pass: entries the pass did not touch belong to agents that have left the
    /// layer, so they are dropped and the route moves on to the next pass.
    async fn finish_pass(&self, route: RouteId, pass: u64) {
        let dropped = {
            let mut memo = self.memo.lock().await;
            let before = memo.len();
            // One pass of grace, not none. A concurrent teardown drain removes keys this tick had
            // already walked past and cannot account for, so a pass can miss an agent that is still
            // in the layer. Dropping it on sight would cost it the sighting it had earned and start
            // its two-tick gate over; holding it one more pass costs a bounded table entry.
            memo.retain(|(memo_route, _), seen| *memo_route != route || seen.pass + 1 >= pass);
            before - memo.len()
        };
        self.passes.lock().await.insert(route, pass + 1);
        if dropped > 0 {
            debug!(
                route = %route,
                dropped,
                "Oplog sweep dropped tracking entries for agents that left the layer"
            );
        }
    }

    async fn forget(&self, route: RouteId, agent_id: &AgentId) {
        self.memo.lock().await.remove(&(route, agent_id.clone()));
    }

    /// Drops memo entries for agents this executor no longer owns, so a reshard does not leave them
    /// behind forever.
    async fn forget_stale(&self, route: RouteId, assignment: &ShardAssignment) {
        self.memo
            .lock()
            .await
            .retain(|(memo_route, agent_id), _| *memo_route != route || owns(assignment, agent_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionStatus;
    use crate::services::oplog::{
        BlobOplogArchiveService, CommitLevel, CompressedOplogArchiveService,
        MultiLayerOplogService, Oplog, OplogService, PrimaryOplogService,
    };
    use crate::services::shard::ShardServiceDefault;
    use crate::storage::indexed::memory::InMemoryIndexedStorage;
    use crate::storage::indexed::{IndexedStorageError, IndexedStorageNamespace, ScanCursor};
    use async_trait::async_trait;
    use golem_common::model::account::AccountId;
    use golem_common::model::application::ApplicationId;
    use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
    use golem_common::model::component_metadata::ComponentMetadata;
    use golem_common::model::oplog::OplogEntry;
    use golem_common::model::{
        AgentFingerprint, AgentInvocation, AgentMetadata, AgentStatusRecord, RetryConfig, Timestamp,
    };
    use golem_common::read_only_lock;
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_service_base::model::component::Component;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use nonempty_collections::nev;
    use std::collections::{BTreeMap, HashSet};
    use std::sync::RwLock;
    use std::time::Duration;
    use test_r::{test, timeout};
    use uuid::Uuid;

    const EPHEMERAL_L1: RouteId = RouteId {
        agent_mode: AgentMode::Ephemeral,
        source_level: 1,
    };

    fn agent(name: &str, component_id: ComponentId) -> AgentId {
        AgentId {
            component_id,
            agent_id: name.to_string(),
        }
    }

    fn create_entry(agent_id: &AgentId, environment_id: EnvironmentId) -> OplogEntry {
        OplogEntry::create(
            agent_id.clone(),
            AgentMode::Ephemeral,
            ComponentRevision::new(1).unwrap(),
            Vec::new(),
            environment_id,
            AccountId::new(),
            None,
            100,
            100,
            HashSet::new(),
            Vec::new(),
            None,
            Uuid::new_v4(),
        )
    }

    // --- pure functions -----------------------------------------------------------------------

    #[test]
    fn parse_agent_id_round_trips_the_layer_key() {
        let agent_id = agent("counter-1", ComponentId::new());
        assert_eq!(parse_agent_id(&agent_id.to_redis_key()), Some(agent_id));
    }

    #[test]
    fn parse_agent_id_keeps_separators_inside_the_agent_name() {
        let agent_id = agent("ns:counter:1", ComponentId::new());
        assert_eq!(parse_agent_id(&agent_id.to_redis_key()), Some(agent_id));
    }

    #[test]
    fn parse_agent_id_rejects_malformed_keys() {
        assert_eq!(parse_agent_id("not-a-uuid:counter"), None);
        assert_eq!(parse_agent_id("no-separator"), None);
        assert_eq!(parse_agent_id(&format!("{}:", Uuid::new_v4())), None);
    }

    #[test]
    fn owns_follows_the_shard_assignment() {
        let agent_id = agent("counter-1", ComponentId::new());
        let shard_id = ShardId::from_routing_hash(ShardId::hash_agent_id(&agent_id), 4);

        let mine = ShardAssignment {
            number_of_shards: 4,
            shard_ids: HashSet::from([shard_id]),
        };
        let theirs = ShardAssignment {
            number_of_shards: 4,
            shard_ids: HashSet::new(),
        };

        assert!(owns(&mine, &agent_id));
        assert!(!owns(&theirs, &agent_id));
    }

    #[test]
    fn an_agent_is_quiet_only_after_its_index_survives_a_pass() {
        let first = OplogIndex::from_u64(11);
        let second = OplogIndex::from_u64(12);
        let seen = |index, pass| Some(Seen { index, pass });

        assert_eq!(assess(None, first, 1), Verdict::Wait);
        assert_eq!(assess(seen(first, 0), second, 1), Verdict::Wait);
        assert_eq!(assess(seen(second, 0), second, 1), Verdict::Move);
    }

    /// A backend may hand the same key back twice inside one walk. Comparing indices alone let the
    /// second sighting satisfy the gate the first had just opened, archiving inside one pass and
    /// skipping the interval the gate exists to impose.
    #[test]
    fn a_sighting_from_the_same_pass_does_not_open_the_gate() {
        let index = OplogIndex::from_u64(11);

        assert_eq!(
            assess(Some(Seen { index, pass: 3 }), index, 3),
            Verdict::Wait,
            "a duplicate key within one pass is not a second sighting"
        );
        assert_eq!(
            assess(Some(Seen { index, pass: 2 }), index, 3),
            Verdict::Move,
            "a sighting from the previous pass is"
        );
    }

    #[test]
    fn a_tick_splits_its_budgets_across_its_routes() {
        let config = OplogSweepConfig {
            max_scanned_per_tick: 100,
            max_archives_per_tick: 10,
            ..OplogSweepConfig::default()
        };

        let mut budget = TickBudget::new(&config, 2);
        assert_eq!(
            budget.share(),
            Some((50, 5)),
            "an even split, not the whole of it"
        );

        // A route that finds nothing leaves its share to the one behind it rather than wasting it.
        budget.spend(0, 0);
        assert_eq!(budget.share(), Some((100, 10)));

        let mut budget = TickBudget::new(&config, 2);
        budget.spend(50, 5);
        assert_eq!(
            budget.share(),
            Some((50, 5)),
            "and a route that spends its share does not"
        );
    }

    /// The share used to floor at one of each, so an exhausted tick still handed every route a
    /// step: four routes and `max_scanned_per_tick = 1` scanned four keys, and an archive phase
    /// that overshot its page did not stop the routes behind it.
    #[test]
    fn a_spent_budget_hands_out_no_more_work() {
        let config = OplogSweepConfig {
            max_scanned_per_tick: 1,
            max_archives_per_tick: 1,
            ..OplogSweepConfig::default()
        };

        let mut budget = TickBudget::new(&config, 4);
        assert_eq!(
            budget.share(),
            Some((1, 1)),
            "the first route gets the one key there is to spend"
        );
        budget.spend(1, 1);
        assert_eq!(
            budget.share(),
            None,
            "and there is nothing left for a second"
        );

        // Overshooting is the same answer. An archive phase stops at the first page that carries
        // it past its bound rather than splitting one, so a route can spend more than it was
        // handed.
        let mut budget = TickBudget::new(&config, 4);
        budget.spend(10, 10);
        assert_eq!(budget.share(), None);

        // Either budget alone is enough to stop the tick: a route with keys left to scan but no
        // archives left would only lengthen the list the next tick inherits.
        let mut budget = TickBudget::new(
            &OplogSweepConfig {
                max_scanned_per_tick: 100,
                max_archives_per_tick: 1,
                ..OplogSweepConfig::default()
            },
            4,
        );
        budget.spend(1, 1);
        assert_eq!(budget.share(), None);
    }

    #[test]
    fn the_backoff_doubles_while_ticks_keep_running_past_their_deadline() {
        assert_eq!(next_backoff(1, true, 8), 2);
        assert_eq!(next_backoff(2, true, 8), 4);
        assert_eq!(next_backoff(4, true, 8), 8);
    }

    #[test]
    fn the_backoff_stops_at_the_cap() {
        assert_eq!(
            next_backoff(8, true, 8),
            8,
            "doubling must not pass the cap"
        );
        assert_eq!(
            next_backoff(u32::MAX, true, 8),
            8,
            "and must not overflow on the way there"
        );
    }

    /// The reset is what keeps a single slow patch from muting the sweep for the rest of the
    /// pod's life. One tick inside its deadline is enough.
    #[test]
    fn one_tick_inside_its_deadline_returns_the_sweep_to_full_pace() {
        assert_eq!(next_backoff(8, false, 8), 1);
        assert_eq!(next_backoff(1, false, 8), 1);
    }

    /// A cap of zero would otherwise multiply the wait to nothing and spin the loop, which is the
    /// failure `MIN_INTERVAL` guards against on the other knob.
    #[test]
    fn a_zero_cap_is_read_as_one_rather_than_disabling_the_wait() {
        assert_eq!(next_backoff(1, true, 0), 1);
        assert_eq!(next_backoff(4, true, 0), 1);
    }

    #[test]
    fn a_route_names_its_mode_and_source_level() {
        assert_eq!(EPHEMERAL_L1.to_string(), "ephemeral-l1");
        assert_eq!(
            RouteId {
                agent_mode: AgentMode::Durable,
                source_level: 2
            }
            .to_string(),
            "durable-l2"
        );
    }

    #[test]
    fn no_setting_can_route_a_durable_layer() {
        // The durable primary hop targets the indexed compressed layer, where re-appending a
        // prefix hits a unique violation that `retry_storage_op` turns into a pod-killing panic.
        // Reaching that has to cost a code change, so the mode list is a const with no setting
        // behind it. This fails the moment one is added.
        let layers = layers();
        let sweeper = build(
            &layers,
            OplogSweepConfig::default(),
            all_shards(),
            EnvironmentId::new(),
            HashSet::new(),
        );

        assert!(
            sweeper.routes.iter().any(|route| route.id == EPHEMERAL_L1),
            "the ephemeral route is not optional either"
        );
        let modes: Vec<AgentMode> = sweeper
            .routes
            .iter()
            .map(|route| route.id.agent_mode)
            .filter(|mode| *mode != AgentMode::Ephemeral)
            .collect();
        assert!(
            modes.is_empty(),
            "routed a mode the archive step cannot survive: {modes:?}"
        );
    }

    #[test]
    fn tally_counts_every_outcome_once() {
        let report = tally([
            Outcome::Unparseable,
            Outcome::NotOwned,
            Outcome::Empty,
            Outcome::Moving,
            Outcome::Resident,
            Outcome::Unaddressable,
            Outcome::ArchiveFailed,
            Outcome::Archived,
            Outcome::Archived,
        ]);

        assert_eq!(report.scanned, 9);
        assert_eq!(report.archived, 2);
        assert_eq!(
            report.unparseable
                + report.not_owned
                + report.empty
                + report.moving
                + report.resident
                + report.unaddressable
                + report.archive_failed
                + report.archived,
            report.scanned
        );
    }

    #[test]
    fn merge_adds_every_field() {
        // Distinct non-zero values on both sides, so a field added wrongly cannot coincide with
        // the right answer.
        let left = RouteReport {
            scanned: 2,
            unparseable: 3,
            not_owned: 4,
            empty: 5,
            moving: 6,
            resident: 7,
            unaddressable: 8,
            archive_failed: 9,
            archived: 10,
            truncated: false,
        };
        let right = RouteReport {
            scanned: 12,
            unparseable: 13,
            not_owned: 14,
            empty: 15,
            moving: 16,
            resident: 17,
            unaddressable: 18,
            archive_failed: 19,
            archived: 20,
            truncated: true,
        };

        let merged = merge(left, right);
        assert_eq!(merged.scanned, 14);
        assert_eq!(merged.unparseable, 16);
        assert_eq!(merged.not_owned, 18);
        assert_eq!(merged.empty, 20);
        assert_eq!(merged.moving, 22);
        assert_eq!(merged.resident, 24);
        assert_eq!(merged.unaddressable, 26);
        assert_eq!(merged.archive_failed, 28);
        assert_eq!(merged.archived, 30);
        assert!(merged.truncated, "truncation is sticky");
    }

    #[test]
    fn triage_settles_what_it_can_without_touching_storage() {
        let component_id = ComponentId::new();
        let mine = agent("mine", component_id);
        let my_shard = ShardId::from_routing_hash(ShardId::hash_agent_id(&mine), 4);
        // Picking a name rather than trusting one: with four shards a second arbitrary name lands
        // on the same one often enough to make the assertion flaky.
        let theirs = (0..)
            .map(|i| agent(&format!("theirs-{i}"), component_id))
            .find(|candidate| {
                ShardId::from_routing_hash(ShardId::hash_agent_id(candidate), 4) != my_shard
            })
            .expect("no agent name maps to another shard");
        let assignment = ShardAssignment {
            number_of_shards: 4,
            shard_ids: HashSet::from([my_shard]),
        };

        let keys = vec![
            mine.to_redis_key(),
            theirs.to_redis_key(),
            "garbage".to_string(),
        ];
        let (candidates, settled) = triage(&keys, &assignment);

        assert_eq!(candidates, vec![mine]);
        assert_eq!(
            settled.iter().filter(|o| **o == Outcome::NotOwned).count(),
            1
        );
        assert_eq!(
            settled
                .iter()
                .filter(|o| **o == Outcome::Unparseable)
                .count(),
            1
        );
    }

    // --- the whole mechanism ------------------------------------------------------------------

    struct Layers {
        oplog_service: Arc<dyn OplogService>,
        archives: Vec<Arc<dyn OplogArchiveService>>,
        indexed_storage: Arc<InMemoryIndexedStorage>,
    }

    fn layers() -> Layers {
        let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
        let blob_storage = Arc::new(InMemoryBlobStorage::new());
        let compressed: Arc<dyn OplogArchiveService> = Arc::new(
            CompressedOplogArchiveService::new(indexed_storage.clone(), 1, RetryConfig::default()),
        );
        let blob: Arc<dyn OplogArchiveService> =
            Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 0));
        Layers {
            oplog_service: Arc::new(MultiLayerOplogService::new(
                Arc::new(futures::executor::block_on(PrimaryOplogService::new(
                    indexed_storage.clone(),
                    blob_storage.clone(),
                    100,
                    100,
                    1024,
                    RetryConfig::default(),
                ))),
                nev![compressed.clone(), blob.clone()],
                // High enough that the size trigger never fires: the sweep is the only thing
                // moving entries in these tests.
                1000,
                1000,
            )),
            archives: vec![compressed, blob],
            indexed_storage,
        }
    }

    /// Hands back a fixed number of keys per page whatever it was asked for, and forwards
    /// everything else. A real backend does both: Redis treats the count as a hint, so a page can
    /// come back short with the walk still live, or longer than the ask.
    #[derive(Debug)]
    struct FixedPages {
        inner: Arc<InMemoryIndexedStorage>,
        keys_per_page: u64,
        /// When set, every walk serves the whole namespace twice before it ends, and the resume
        /// token is ignored. That is the smallest shape of what Redis `SCAN` is allowed to do: a
        /// key present throughout is returned at least once, not exactly once.
        duplicate_pages: bool,
        /// Cancelled once a page has been served, then disarmed. That is where a tick deadline
        /// lands when probing one page of keys outlasts it, which is the whole reason the deadline
        /// exists: see `OplogSweepConfig::max_tick_duration`.
        cancel_on_scan: std::sync::Mutex<Option<CancellationToken>>,
        calls: std::sync::atomic::AtomicU64,
    }

    #[async_trait]
    impl IndexedStorage for FixedPages {
        async fn number_of_replicas(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
        ) -> Result<u8, IndexedStorageError> {
            self.inner.number_of_replicas(svc_name, api_name).await
        }

        async fn wait_for_replicas(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            replicas: u8,
            timeout: Duration,
        ) -> Result<u8, IndexedStorageError> {
            self.inner
                .wait_for_replicas(svc_name, api_name, replicas, timeout)
                .await
        }

        async fn exists(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
        ) -> Result<bool, IndexedStorageError> {
            self.inner.exists(svc_name, api_name, namespace, key).await
        }

        async fn scan(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            namespace: IndexedStorageMetaNamespace,
            prefix: Option<&str>,
            cursor: ScanCursor,
            count: u64,
        ) -> Result<(ScanCursor, Vec<String>), IndexedStorageError> {
            self.inner
                .scan(svc_name, api_name, namespace, prefix, cursor, count)
                .await
        }

        async fn scan_stable(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            namespace: IndexedStorageMetaNamespace,
            prefix: Option<&str>,
            resume: Option<ScanResume>,
            _count: u64,
        ) -> Result<(Option<ScanResume>, Vec<String>), IndexedStorageError> {
            if self.duplicate_pages {
                let (_, keys) = self
                    .inner
                    .scan_stable(svc_name, api_name, namespace, prefix, None, 10_000)
                    .await?;
                if let Some(token) = self.cancel_on_scan.lock().unwrap().take() {
                    token.cancel();
                }
                let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                return if call.is_multiple_of(2) {
                    Ok((Some(ScanResume::Marker("again".to_string())), keys))
                } else {
                    Ok((None, keys))
                };
            }
            let page = self
                .inner
                .scan_stable(
                    svc_name,
                    api_name,
                    namespace,
                    prefix,
                    resume,
                    self.keys_per_page,
                )
                .await;
            if let Some(token) = self.cancel_on_scan.lock().unwrap().take() {
                token.cancel();
            }
            page
        }

        async fn append(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
            id: u64,
            value: Vec<u8>,
        ) -> Result<(), IndexedStorageError> {
            self.inner
                .append(svc_name, api_name, entity_name, namespace, key, id, value)
                .await
        }

        async fn length(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
        ) -> Result<u64, IndexedStorageError> {
            self.inner.length(svc_name, api_name, namespace, key).await
        }

        async fn delete(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
        ) -> Result<(), IndexedStorageError> {
            self.inner.delete(svc_name, api_name, namespace, key).await
        }

        async fn read(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
            start_id: u64,
            end_id: u64,
        ) -> Result<Vec<(u64, Vec<u8>)>, IndexedStorageError> {
            self.inner
                .read(
                    svc_name,
                    api_name,
                    entity_name,
                    namespace,
                    key,
                    start_id,
                    end_id,
                )
                .await
        }

        async fn first(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
        ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
            self.inner
                .first(svc_name, api_name, entity_name, namespace, key)
                .await
        }

        async fn last(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
        ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
            self.inner
                .last(svc_name, api_name, entity_name, namespace, key)
                .await
        }

        async fn last_id(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
        ) -> Result<Option<u64>, IndexedStorageError> {
            self.inner
                .last_id(svc_name, api_name, entity_name, namespace, key)
                .await
        }

        async fn closest(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
            id: u64,
        ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
            self.inner
                .closest(svc_name, api_name, entity_name, namespace, key, id)
                .await
        }

        async fn drop_prefix(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
            last_dropped_id: u64,
        ) -> Result<(), IndexedStorageError> {
            self.inner
                .drop_prefix(svc_name, api_name, namespace, key, last_dropped_id)
                .await
        }
    }

    /// A stack with more layers below its source than one archive pass is allowed to walk.
    fn deep_layers(compressed_levels: usize) -> Layers {
        let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
        let blob_storage = Arc::new(InMemoryBlobStorage::new());
        let mut archives: Vec<Arc<dyn OplogArchiveService>> = (1..=compressed_levels)
            .map(|level| {
                Arc::new(CompressedOplogArchiveService::new(
                    indexed_storage.clone(),
                    level,
                    RetryConfig::default(),
                )) as Arc<dyn OplogArchiveService>
            })
            .collect();
        archives.push(Arc::new(BlobOplogArchiveService::new(
            blob_storage.clone(),
            0,
        )));
        let mut stack = nev![archives[0].clone()];
        for archive in archives.iter().skip(1) {
            stack.push(archive.clone());
        }
        Layers {
            oplog_service: Arc::new(MultiLayerOplogService::new(
                Arc::new(futures::executor::block_on(PrimaryOplogService::new(
                    indexed_storage.clone(),
                    blob_storage.clone(),
                    100,
                    100,
                    1024,
                    RetryConfig::default(),
                ))),
                stack,
                1000,
                1000,
            )),
            archives,
            indexed_storage,
        }
    }

    fn metadata(agent_id: &AgentId, environment_id: EnvironmentId) -> AgentMetadata {
        AgentMetadata {
            agent_id: agent_id.clone(),
            env: vec![],
            environment_id,
            created_by: AccountId::new(),
            config: Vec::new(),
            created_at: Timestamp::now_utc(),
            parent: None,
            last_known_status: AgentStatusRecord::default(),
            original_phantom_id: None,
            fingerprint: AgentFingerprint::new(),
            agent_mode: AgentMode::Ephemeral,
        }
    }

    fn status_lock() -> read_only_lock::tokio::ReadOnlyLock<AgentStatusRecord> {
        read_only_lock::tokio::ReadOnlyLock::new(Arc::new(tokio::sync::RwLock::new(
            AgentStatusRecord::default(),
        )))
    }

    fn execution_lock() -> read_only_lock::std::ReadOnlyLock<ExecutionStatus> {
        read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(ExecutionStatus::Suspended {
            agent_mode: AgentMode::Ephemeral,
            timestamp: Timestamp::now_utc(),
        })))
    }

    /// Resolves every component to one environment, which is what the real component service does
    /// for the agents of a given component.
    struct FixedEnvironment {
        environment_id: EnvironmentId,
        /// Answers nothing at all, deployed lookup or pinned.
        fails: bool,
        /// Answers only a pinned revision, which is what a deleted component looks like:
        /// `get_deployed_component_metadata` skips it, `get_component_metadata` still has it.
        deleted: bool,
        /// Lookups that reached this far, so a test can prove the sweeper stopped repaying them.
        lookups: std::sync::atomic::AtomicU64,
    }

    #[async_trait]
    impl ComponentService for FixedEnvironment {
        async fn get(
            &self,
            _engine: &wasmtime::Engine,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
            unreachable!("the sweep never loads a component")
        }

        async fn get_metadata(
            &self,
            component_id: ComponentId,
            forced_revision: Option<ComponentRevision>,
        ) -> Result<Component, WorkerExecutorError> {
            self.lookups
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.fails || (self.deleted && forced_revision.is_none()) {
                return Err(WorkerExecutorError::runtime("component not found"));
            }
            Ok(Component {
                id: component_id,
                revision: ComponentRevision::INITIAL,
                environment_id: self.environment_id,
                component_name: ComponentName("sweep-test".to_string()),
                hash: golem_common::model::diff::Hash::empty(),
                application_id: ApplicationId::new(),
                account_id: AccountId::new(),
                component_size: 100,
                metadata: ComponentMetadata::from_parts(
                    Default::default(),
                    vec![],
                    None,
                    None,
                    vec![],
                    BTreeMap::new(),
                ),
                created_at: chrono::Utc::now(),
                wasm_hash: golem_common::model::diff::Hash::empty(),
                object_store_key: String::new(),
            })
        }

        async fn resolve_component(
            &self,
            _component_reference: String,
            _resolving_environment: EnvironmentId,
            _resolving_application: ApplicationId,
            _resolving_account: AccountId,
        ) -> Result<Option<ComponentId>, WorkerExecutorError> {
            Ok(None)
        }

        async fn all_cached_metadata(&self) -> Vec<Component> {
            Vec::new()
        }

        async fn invalidate_all_metadata_for_environment(&self, _environment_id: EnvironmentId) {}
    }

    /// Opens the oplog directly instead of building a `Worker` around it. The production adapter is
    /// `Arc<dyn WorkerActivator<Ctx>>`, whose `open_oplog` goes through `get_or_create_suspended`;
    /// both hand the sweep the same `Arc<dyn Oplog>`.
    struct DirectAccess {
        oplog_service: Arc<dyn OplogService>,
        resident: HashSet<AgentId>,
    }

    #[async_trait]
    impl SchedulerWorkerAccess for DirectAccess {
        async fn active_worker_fingerprint(
            &self,
            owned_agent_id: &OwnedAgentId,
        ) -> Option<AgentFingerprint> {
            self.resident
                .contains(&owned_agent_id.agent_id)
                .then(AgentFingerprint::new)
        }

        async fn activate_worker(
            &self,
            _owned_agent_id: &OwnedAgentId,
        ) -> Result<(), WorkerExecutorError> {
            unreachable!("the sweep never activates an agent")
        }

        async fn open_oplog(
            &self,
            owned_agent_id: &OwnedAgentId,
        ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
            Ok(self
                .oplog_service
                .open(
                    owned_agent_id,
                    AgentMode::Ephemeral,
                    None,
                    metadata(&owned_agent_id.agent_id, owned_agent_id.environment_id),
                    status_lock(),
                    execution_lock(),
                )
                .await)
        }

        async fn enqueue_invocation(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _invocation: AgentInvocation,
        ) -> Result<(), WorkerExecutorError> {
            unreachable!("the sweep never enqueues invocations")
        }
    }

    fn all_shards() -> Arc<ShardServiceDefault> {
        let shard_service = Arc::new(ShardServiceDefault::new());
        shard_service.register(1, &HashSet::from([ShardId::new(0)]));
        shard_service
    }

    fn build(
        layers: &Layers,
        config: OplogSweepConfig,
        shards: Arc<dyn ShardService>,
        environment_id: EnvironmentId,
        resident: HashSet<AgentId>,
    ) -> Arc<OplogSweeper> {
        build_over(
            layers.indexed_storage.clone(),
            layers,
            config,
            shards,
            environment_id,
            resident,
        )
    }

    /// The same, over a storage the test supplies, for the pages a backend can hand back.
    fn build_over(
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        layers: &Layers,
        config: OplogSweepConfig,
        shards: Arc<dyn ShardService>,
        environment_id: EnvironmentId,
        resident: HashSet<AgentId>,
    ) -> Arc<OplogSweeper> {
        OplogSweeper::over_layers(
            config,
            indexed_storage,
            &layers.archives,
            shards,
            Arc::new(FixedEnvironment {
                environment_id,
                fails: false,
                deleted: false,
                lookups: std::sync::atomic::AtomicU64::new(0),
            }),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                resident,
            }),
        )
    }

    /// No tick loop: the test drives `sweep_once` itself.
    fn manual() -> OplogSweepConfig {
        OplogSweepConfig {
            enabled: false,
            ..OplogSweepConfig::default()
        }
    }

    /// Leaves an ephemeral oplog in the compressed layer with nothing to drain it, which is the
    /// state a crash between the last commit and `archive_ephemeral_oplog` leaves behind.
    async fn stranded_ephemeral_oplog(
        layers: &Layers,
        agent_id: &AgentId,
        environment_id: EnvironmentId,
    ) {
        let owned_agent_id = OwnedAgentId::new(environment_id, agent_id);
        let oplog = layers
            .oplog_service
            .create(
                &owned_agent_id,
                AgentMode::Ephemeral,
                create_entry(agent_id, environment_id),
                metadata(agent_id, environment_id),
                status_lock(),
                execution_lock(),
            )
            .await;
        oplog.add(OplogEntry::suspend()).await;
        oplog.add(OplogEntry::exited()).await;
        oplog.commit(CommitLevel::Always).await;
        drop(oplog);
    }

    #[test]
    async fn the_bottom_layer_is_never_a_source() {
        let layers = layers();
        let sweeper = build(
            &layers,
            manual(),
            all_shards(),
            EnvironmentId::new(),
            HashSet::new(),
        );

        // Two archives, one route: the blob layer receives entries and cannot enumerate its own.
        assert_eq!(sweeper.routes.len(), 1);
        assert_eq!(sweeper.routes[0].id, EPHEMERAL_L1);
    }

    #[test]
    #[timeout("1m")]
    async fn a_quiet_ephemeral_layer_is_archived_on_the_second_tick() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        let sweeper = build(
            &layers,
            manual(),
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        // An ephemeral oplog never reaches the primary: its entries start in the first lower layer,
        // and the layer below is empty.
        assert_eq!(
            layers.archives[0]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::from_u64(3)
        );
        assert_eq!(
            layers.archives[1]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::NONE
        );

        // The first tick has nothing to compare against, so it only records the index.
        let first = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(first.scanned(), 1);
        assert_eq!(first.route(EPHEMERAL_L1).moving, 1);
        assert_eq!(first.archived(), 0);

        // The index has not moved, so the second tick archives.
        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(second.archived(), 1);

        // The entries changed layer, which is what "archived" has to mean.
        assert_eq!(
            layers.archives[0]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::NONE
        );
        assert_eq!(
            layers.archives[1]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::from_u64(3)
        );

        // An archived agent is dropped from the tracking table rather than left to grow it.
        assert!(sweeper.memo.lock().await.is_empty());

        // `drop_prefix` removes the key, so the layer no longer enumerates the agent at all.
        let third = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(third.scanned(), 0);

        // Nothing was lost on the way down.
        let entries = layers
            .oplog_service
            .read(
                &owned_agent_id,
                AgentMode::Ephemeral,
                OplogIndex::INITIAL,
                3,
            )
            .await;
        assert_eq!(entries.len(), 3);
        assert!(matches!(
            entries.get(&OplogIndex::INITIAL),
            Some(OplogEntry::Create { .. })
        ));
    }

    /// Regression: an expired tick used to drop every candidate it had not reached while still
    /// saving the scan cursor it had advanced, so a store slow enough to cut every tick during
    /// probing gave a sweep that walked its namespace forever and archived nothing. That store is
    /// the one condition the deadline exists for, so the deadline disabled the sweep exactly when
    /// it was needed.
    #[test]
    #[timeout("1m")]
    async fn a_tick_cut_short_before_archiving_keeps_its_candidates() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

        let storage = Arc::new(FixedPages {
            inner: layers.indexed_storage.clone(),
            keys_per_page: 10,
            duplicate_pages: false,
            cancel_on_scan: std::sync::Mutex::new(None),
            calls: std::sync::atomic::AtomicU64::new(0),
        });
        let sweeper = build_over(
            storage.clone(),
            &layers,
            manual(),
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        // Unarmed: the first tick only records the index, and has nothing to archive anyway.
        let first = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(first.route(EPHEMERAL_L1).moving, 1);
        assert!(sweeper.deferred.lock().await.is_empty());

        // Armed. The gate is open now, so this tick probes the agent and then loses its deadline
        // before it can archive it.
        let cut = CancellationToken::new();
        *storage.cancel_on_scan.lock().unwrap() = Some(cut.clone());
        let second = sweeper.sweep_once(&cut).await;
        assert_eq!(second.archived(), 0, "the tick ran out of time to archive");
        assert!(second.route(EPHEMERAL_L1).truncated);
        assert_eq!(
            sweeper.deferred.lock().await.get(&EPHEMERAL_L1),
            Some(&vec![agent_id.clone()]),
            "but the probe it paid for is kept"
        );
        assert_eq!(
            layers.archives[0]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::from_u64(3),
            "and nothing moved"
        );

        // Cut in the same place again. The carried candidate is archived before the scan gets the
        // chance to spend this tick's deadline the same way, so a tick that keeps timing out
        // still makes progress.
        let cut_again = CancellationToken::new();
        *storage.cancel_on_scan.lock().unwrap() = Some(cut_again.clone());
        let third = sweeper.sweep_once(&cut_again).await;
        assert_eq!(third.archived(), 1);
        assert!(sweeper.deferred.lock().await.is_empty());
        assert_eq!(
            third.scanned(),
            0,
            "and the tick that scanned it paid for the walk, so this one is not charged again"
        );
        assert_eq!(
            layers.archives[0]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::NONE
        );
        assert_eq!(
            layers.archives[1]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::from_u64(3)
        );
    }

    /// Regression: `environment_of` used to pay a registry lookup per component per tick, two of
    /// them for a deleted component. A tick that kept losing its deadline inside that loop
    /// archived nothing and learned nothing, so it wrote back the same carried list and the next
    /// tick started from exactly the same place. Remembering the answer is what bounds that.
    #[test]
    #[timeout("1m")]
    async fn an_environment_is_resolved_once_and_not_once_per_tick() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        // Deleted, so a cold lookup costs two calls rather than one: the deployed lookup misses
        // before the pinned one answers.
        let components = Arc::new(FixedEnvironment {
            environment_id,
            fails: false,
            deleted: true,
            lookups: std::sync::atomic::AtomicU64::new(0),
        });
        for name in ["counter-1", "counter-2"] {
            stranded_ephemeral_oplog(&layers, &agent(name, component_id), environment_id).await;
        }

        let sweeper = OplogSweeper::over_layers(
            manual(),
            layers.indexed_storage.clone(),
            &layers.archives,
            all_shards(),
            components.clone(),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                resident: HashSet::new(),
            }),
        );

        // The first tick only records indexes, so it resolves nothing.
        sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(
            components.lookups.load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        // The second archives both agents, and they share one component.
        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(second.archived(), 2);
        let after_first_resolve = components.lookups.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            after_first_resolve, 2,
            "one deployed miss and one pinned hit, for the component both agents share"
        );

        // A later agent on the same component asks nothing further.
        stranded_ephemeral_oplog(&layers, &agent("counter-3", component_id), environment_id).await;
        sweeper.sweep_once(&CancellationToken::new()).await;
        let third = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(third.archived(), 1);
        assert_eq!(
            components.lookups.load(std::sync::atomic::Ordering::SeqCst),
            after_first_resolve,
            "the environment was already known"
        );
    }

    /// The scanned path drops what this executor no longer owns in `triage`. A carried candidate
    /// skips that, because it was triaged a tick ago, and archiving an agent this pod may no
    /// longer own races the pod that took the shard: `archive_agent`'s mutual exclusion is a
    /// suspended `Worker` in this process, which the new owner cannot see.
    #[test]
    #[timeout("1m")]
    async fn a_carried_candidate_is_dropped_when_its_shard_moves_away() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

        let storage = Arc::new(FixedPages {
            inner: layers.indexed_storage.clone(),
            keys_per_page: 10,
            duplicate_pages: false,
            cancel_on_scan: std::sync::Mutex::new(None),
            calls: std::sync::atomic::AtomicU64::new(0),
        });
        let shards = all_shards();
        let sweeper = build_over(
            storage.clone(),
            &layers,
            manual(),
            shards.clone(),
            environment_id,
            HashSet::new(),
        );

        sweeper.sweep_once(&CancellationToken::new()).await;
        let cut = CancellationToken::new();
        *storage.cancel_on_scan.lock().unwrap() = Some(cut.clone());
        sweeper.sweep_once(&cut).await;
        assert_eq!(
            sweeper.deferred.lock().await.get(&EPHEMERAL_L1),
            Some(&vec![agent_id.clone()]),
            "the tick ran out of time holding this candidate"
        );

        // The shard moves to another executor before the carried candidate is reached.
        shards
            .set_shard_assignment(4, &HashSet::new())
            .expect("assignment");
        let after = sweeper.sweep_once(&CancellationToken::new()).await;

        assert_eq!(after.archived(), 0);
        assert_eq!(
            after.route(EPHEMERAL_L1).not_owned,
            2,
            "once for the carried candidate this tick declined, and once for the key still in the \
             layer when the scan reached it"
        );
        assert!(sweeper.deferred.lock().await.is_empty());
        assert_eq!(
            layers.archives[0]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::from_u64(3),
            "the entries stay for whoever owns the shard now"
        );
    }

    /// Only these two open the oplog. Charging the archive budget for the rest let a carried list
    /// of agents that turned out to be resident exhaust the allowance and stop the scan running.
    #[test]
    fn only_the_outcomes_that_reached_the_store_cost_archive_budget() {
        assert_eq!(
            archive_work(&[Outcome::Archived, Outcome::ArchiveFailed, Outcome::Archived]),
            3
        );
        assert_eq!(
            archive_work(&[
                Outcome::Resident,
                Outcome::Empty,
                Outcome::NotOwned,
                Outcome::Unaddressable,
                Outcome::Moving,
                Outcome::Unparseable,
            ]),
            0
        );
    }

    /// A tick that stops early used to leave the routes it never reached to be skipped again by
    /// the next tick, and the one before that, for as long as the budget or the deadline kept
    /// running out in the same place.
    #[test]
    #[timeout("1m")]
    async fn a_tick_that_stops_early_starts_the_next_one_where_it_stopped() {
        let layers = deep_layers(2);
        let environment_id = EnvironmentId::new();
        let storage = Arc::new(FixedPages {
            inner: layers.indexed_storage.clone(),
            keys_per_page: 10,
            duplicate_pages: false,
            cancel_on_scan: std::sync::Mutex::new(None),
            calls: std::sync::atomic::AtomicU64::new(0),
        });
        let sweeper = build_over(
            storage.clone(),
            &layers,
            manual(),
            all_shards(),
            environment_id,
            HashSet::new(),
        );
        assert_eq!(
            sweeper.routes.len(),
            2,
            "two compressed sources over a blob bottom"
        );

        // Cut during the first route, so the second is never reached.
        let cut = CancellationToken::new();
        *storage.cancel_on_scan.lock().unwrap() = Some(cut.clone());
        let first = sweeper.sweep_once(&cut).await;
        assert_eq!(first.routes.len(), 1);
        assert_eq!(first.routes[0].0, sweeper.routes[0].id);
        assert_eq!(
            *sweeper.route_cursor.lock().await,
            1,
            "so the next tick begins with the route this one skipped"
        );

        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(second.routes.len(), 1);
        assert_eq!(
            second.routes[0].0, sweeper.routes[1].id,
            "the skipped route, not the one that already ran"
        );
        assert_eq!(
            *sweeper.route_cursor.lock().await,
            0,
            "and a tick that reaches the end of the stack starts the next one at the top"
        );

        // Route order inside a tick is untouched: the deepest source still runs first, so a tick
        // never hands entries to a layer it is about to drain.
        let third = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(
            third.routes.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![sweeper.routes[0].id, sweeper.routes[1].id]
        );
    }

    #[test]
    #[timeout("1m")]
    async fn an_agent_invoked_a_second_time_is_still_archived() {
        // Regression: the first archive step moves the `Create` entry down a layer, and a second
        // invocation continues the same index space. So the layer holds entries starting above
        // `OplogIndex::INITIAL` with no `Create` in it, which is the ordinary state for any
        // ephemeral agent invoked more than once, and it used to make the sweep skip that agent
        // forever.
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        let sweeper = build(
            &layers,
            manual(),
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(
            sweeper
                .sweep_once(&CancellationToken::new())
                .await
                .archived(),
            1,
            "first lifetime"
        );

        let oplog = layers
            .oplog_service
            .open(
                &owned_agent_id,
                AgentMode::Ephemeral,
                None,
                metadata(&agent_id, environment_id),
                status_lock(),
                execution_lock(),
            )
            .await;
        oplog.add(OplogEntry::suspend()).await;
        oplog.add(OplogEntry::exited()).await;
        oplog.commit(CommitLevel::Always).await;
        drop(oplog);

        let stranded = layers.archives[0]
            .read(
                &owned_agent_id,
                AgentMode::Ephemeral,
                OplogIndex::INITIAL,
                1,
            )
            .await;
        assert!(
            stranded.is_empty(),
            "the second lifetime must not start at OplogIndex::INITIAL, or this proves nothing"
        );

        sweeper.sweep_once(&CancellationToken::new()).await;
        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(second.route(EPHEMERAL_L1).unaddressable, 0);
        assert_eq!(second.archived(), 1, "second lifetime");
        assert_eq!(
            layers.archives[0]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::NONE
        );
    }

    #[test]
    async fn an_agent_whose_component_cannot_be_resolved_is_reported_not_skipped_silently() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;

        let sweeper = OplogSweeper::over_layers(
            manual(),
            layers.indexed_storage.clone(),
            &layers.archives,
            all_shards(),
            Arc::new(FixedEnvironment {
                environment_id,
                fails: true,
                deleted: false,
                lookups: std::sync::atomic::AtomicU64::new(0),
            }),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                resident: HashSet::new(),
            }),
        );

        sweeper.sweep_once(&CancellationToken::new()).await;
        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(second.route(EPHEMERAL_L1).unaddressable, 1);
        assert_eq!(second.archived(), 0);
    }

    /// A stranded oplog is what the sweep exists for, and the component behind one is exactly the
    /// component most likely to have been deleted in the meantime. Resolving only through the
    /// deployed lookup made those oplogs permanently unarchiveable: every tick reported
    /// `unaddressable` and left the entries where they were.
    #[test]
    async fn an_agent_whose_component_was_deleted_is_still_archived() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;

        let sweeper = OplogSweeper::over_layers(
            manual(),
            layers.indexed_storage.clone(),
            &layers.archives,
            all_shards(),
            Arc::new(FixedEnvironment {
                environment_id,
                fails: false,
                deleted: true,
                lookups: std::sync::atomic::AtomicU64::new(0),
            }),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                resident: HashSet::new(),
            }),
        );

        sweeper.sweep_once(&CancellationToken::new()).await;
        let second = sweeper.sweep_once(&CancellationToken::new()).await;

        assert_eq!(second.route(EPHEMERAL_L1).unaddressable, 0);
        assert_eq!(second.archived(), 1);
        assert_eq!(
            layers.archives[0]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::NONE
        );
    }

    /// Redis `SCAN` promises a key present throughout the walk is returned *at least* once. The
    /// gate used to compare indices alone, so the second sighting satisfied the gate the first had
    /// just opened and the agent was archived inside one pass, skipping the interval entirely.
    #[test]
    async fn a_key_handed_back_twice_in_one_pass_does_not_clear_the_gate() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;

        let sweeper = build_over(
            Arc::new(FixedPages {
                inner: layers.indexed_storage.clone(),
                keys_per_page: 1,
                duplicate_pages: true,
                cancel_on_scan: std::sync::Mutex::new(None),
                calls: std::sync::atomic::AtomicU64::new(0),
            }),
            &layers,
            manual(),
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        // One tick, two pages, the same key on both. Both are sightings from the pass now under
        // way, so neither opens the gate.
        let first = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(first.route(EPHEMERAL_L1).moving, 2);
        assert_eq!(
            first.archived(),
            0,
            "a duplicate satisfied the gate the pass had just opened"
        );

        // The pass ended, so the next one carries a sighting from before it and the gate opens.
        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(second.archived(), 1);
    }

    #[test]
    async fn an_agent_that_cannot_be_archived_keeps_the_gate_it_passed() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;

        // An agent that passes the quiet gate and then fails to archive, here because its component
        // never resolves, stays in the layer. It has to keep the sighting that got it through the
        // gate: if the pass that archives it does not restamp the entry, `finish_pass` eventually
        // drops it and the agent starts the two-tick gate over, on every pass, forever.
        let sweeper = OplogSweeper::over_layers(
            manual(),
            layers.indexed_storage.clone(),
            &layers.archives,
            all_shards(),
            Arc::new(FixedEnvironment {
                environment_id,
                fails: true,
                deleted: false,
                lookups: std::sync::atomic::AtomicU64::new(0),
            }),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                resident: HashSet::new(),
            }),
        );

        // The first tick only records the index; every tick after it should reach the archive and
        // fail there, never fall back to being a fresh sighting.
        sweeper.sweep_once(&CancellationToken::new()).await;
        for tick in 2..=5 {
            let report = sweeper.sweep_once(&CancellationToken::new()).await;
            assert_eq!(
                report.route(EPHEMERAL_L1).unaddressable,
                1,
                "tick {tick} should still be trying to archive"
            );
            assert_eq!(
                report.route(EPHEMERAL_L1).moving,
                0,
                "tick {tick} lost the sighting and restarted the quiet gate"
            );
        }
    }

    #[test]
    async fn an_agent_this_executor_is_running_is_left_alone() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;
        let sweeper = build(
            &layers,
            manual(),
            all_shards(),
            environment_id,
            HashSet::from([agent_id.clone()]),
        );

        // Residency is decided in memory before anything is read, so it lands on the first tick
        // rather than after the two-tick quiet gate.
        let first = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(first.route(EPHEMERAL_L1).resident, 1);
        assert_eq!(first.archived(), 0);

        // And nothing about a running agent is remembered, because it was never a candidate.
        assert!(sweeper.memo.lock().await.is_empty());

        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(second.route(EPHEMERAL_L1).resident, 1);
        assert_eq!(second.archived(), 0);
    }

    /// Appends to an agent's oplog so its last index moves, which is what keeps it failing the
    /// quiet gate from one tick to the next.
    async fn keep_moving(layers: &Layers, agent_id: &AgentId, environment_id: EnvironmentId) {
        let oplog = layers
            .oplog_service
            .open(
                &OwnedAgentId::new(environment_id, agent_id),
                AgentMode::Ephemeral,
                None,
                metadata(agent_id, environment_id),
                status_lock(),
                execution_lock(),
            )
            .await;
        oplog.add(OplogEntry::suspend()).await;
        oplog.commit(CommitLevel::Always).await;
        drop(oplog);
    }

    #[test]
    async fn a_completed_pass_drops_the_agents_that_left_the_layer() {
        // Ephemeral agent ids are unbounded: an invocation with no phantom id gets a fresh
        // `Uuid::new_v4()`. An agent the sweep sees once and that is then drained by its own
        // teardown never appears again, so without this its tracking entry would live until the
        // table was cleared wholesale, which is also what would stop anything from ever being
        // archived.
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        let transient = agent("counter-drained", component_id);
        let staying = agent("counter-staying", component_id);
        stranded_ephemeral_oplog(&layers, &transient, environment_id).await;
        stranded_ephemeral_oplog(&layers, &staying, environment_id).await;

        let sweeper = build(
            &layers,
            manual(),
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(sweeper.memo.lock().await.len(), 2, "both were remembered");

        // The transient agent's own teardown drains its layer, exactly as
        // `archive_ephemeral_oplog` does, and its key disappears.
        layers.archives[0]
            .delete(
                &OwnedAgentId::new(environment_id, &transient),
                AgentMode::Ephemeral,
            )
            .await;

        // The other agent keeps working, so every pass sees it and it stays tracked.
        keep_moving(&layers, &staying, environment_id).await;
        sweeper.sweep_once(&CancellationToken::new()).await;

        assert_eq!(
            sweeper.memo.lock().await.len(),
            2,
            "a pass gets one pass of grace: a concurrent drain can remove a key a tick had already \
             walked past, so the first pass that misses an agent does not condemn it"
        );

        keep_moving(&layers, &staying, environment_id).await;
        sweeper.sweep_once(&CancellationToken::new()).await;

        let memo = sweeper.memo.lock().await;
        assert_eq!(
            memo.len(),
            1,
            "the drained agent must not be tracked forever"
        );
        assert!(memo.contains_key(&(EPHEMERAL_L1, staying)));
    }

    #[test]
    async fn an_agent_on_another_shard_is_left_alone() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;

        let shards = Arc::new(ShardServiceDefault::new());
        shards.register(4, &HashSet::new());
        let sweeper = build(&layers, manual(), shards, environment_id, HashSet::new());

        sweeper.sweep_once(&CancellationToken::new()).await;
        let second = sweeper.sweep_once(&CancellationToken::new()).await;

        assert_eq!(second.route(EPHEMERAL_L1).not_owned, 1);
        assert_eq!(second.archived(), 0);
    }

    #[test]
    async fn a_reshard_drops_the_tracking_entries_it_no_longer_owns() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;

        let shards = all_shards();
        let sweeper = build(
            &layers,
            manual(),
            shards.clone(),
            environment_id,
            HashSet::new(),
        );

        sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(sweeper.memo.lock().await.len(), 1);

        // The shard moves to another executor before the agent ever went quiet for us.
        shards
            .set_shard_assignment(4, &HashSet::new())
            .expect("assignment");
        sweeper.sweep_once(&CancellationToken::new()).await;

        assert!(
            sweeper.memo.lock().await.is_empty(),
            "a tracking entry for an agent we no longer own must not be kept"
        );
    }

    #[test]
    async fn a_budgeted_tick_resumes_where_it_stopped() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        let agents: Vec<AgentId> = (0..3)
            .map(|i| agent(&format!("counter-{i}"), component_id))
            .collect();
        for agent_id in &agents {
            stranded_ephemeral_oplog(&layers, agent_id, environment_id).await;
        }

        // One key per scan call, one key per tick: three ticks to see three agents. Nothing is
        // archived in these ticks, so the scan cursor stays valid across them.
        let sweeper = build(
            &layers,
            OplogSweepConfig {
                enabled: false,
                page_size: 1,
                max_scanned_per_tick: 1,
                ..OplogSweepConfig::default()
            },
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        let mut seen = 0;
        for _ in 0..3 {
            let report = sweeper.sweep_once(&CancellationToken::new()).await;
            assert!(report.route(EPHEMERAL_L1).truncated);
            seen += report.route(EPHEMERAL_L1).scanned;
        }

        assert_eq!(
            seen, 3,
            "each tick carried on rather than revisiting the first"
        );
        assert_eq!(sweeper.memo.lock().await.len(), 3);
    }

    #[test]
    #[timeout("1m")]
    async fn the_archive_budget_stops_a_tick() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        for i in 0..2 {
            stranded_ephemeral_oplog(
                &layers,
                &agent(&format!("counter-{i}"), component_id),
                environment_id,
            )
            .await;
        }

        let sweeper = build(
            &layers,
            OplogSweepConfig {
                enabled: false,
                page_size: 1,
                max_archives_per_tick: 1,
                ..OplogSweepConfig::default()
            },
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        // Two ticks to make both agents quiet, then a tick that may archive only one of them.
        sweeper.sweep_once(&CancellationToken::new()).await;
        sweeper.sweep_once(&CancellationToken::new()).await;
        let budgeted = sweeper.sweep_once(&CancellationToken::new()).await;

        assert_eq!(budgeted.archived(), 1);
        assert!(budgeted.route(EPHEMERAL_L1).truncated);
    }

    #[test]
    #[timeout("1m")]
    async fn one_tick_archives_every_quiet_agent_across_pages() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        for i in 0..4 {
            stranded_ephemeral_oplog(
                &layers,
                &agent(&format!("counter-{i}"), component_id),
                environment_id,
            )
            .await;
        }

        // One key per page, so a tick pages four times before it archives anything. Every agent
        // the scan walked has to be archived, not just the ones on the page the tick happened to
        // stop on.
        let sweeper = build(
            &layers,
            OplogSweepConfig {
                enabled: false,
                page_size: 1,
                ..OplogSweepConfig::default()
            },
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        let first = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(first.archived(), 0, "nothing is quiet on a first sighting");
        assert_eq!(first.route(EPHEMERAL_L1).moving, 4);

        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(
            second.archived(),
            4,
            "every agent the scan walked was archived, not just the first page's"
        );
        assert!(sweeper.memo.lock().await.is_empty());
    }

    #[test]
    #[timeout("1m")]
    async fn a_full_tracking_table_declines_an_agent_rather_than_forgetting_every_agent() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        for i in 0..2 {
            stranded_ephemeral_oplog(
                &layers,
                &agent(&format!("counter-{i}"), component_id),
                environment_id,
            )
            .await;
        }

        // Room for one agent and two agents to track. Clearing the table on overflow would drop the
        // sighting the first agent already had, and because an agent needs two sightings at the
        // same index to qualify, neither would ever reach a second one.
        let sweeper = build(
            &layers,
            OplogSweepConfig {
                enabled: false,
                max_tracked_agents: 1,
                ..OplogSweepConfig::default()
            },
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        let first = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(first.archived(), 0);
        assert_eq!(sweeper.memo.lock().await.len(), 1, "the bound holds");

        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(
            second.archived(),
            1,
            "the agent that was tracked kept its sighting and qualified"
        );
    }

    #[test]
    async fn a_tick_never_scans_past_its_budget() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        for i in 0..10 {
            stranded_ephemeral_oplog(
                &layers,
                &agent(&format!("counter-{i}"), component_id),
                environment_id,
            )
            .await;
        }

        // A page far larger than the budget, so the tick has to ask the scan for less than a full
        // page. Trimming the page afterwards would advance the cursor past keys nothing read.
        let sweeper = build(
            &layers,
            OplogSweepConfig {
                enabled: false,
                page_size: 128,
                max_scanned_per_tick: 3,
                ..OplogSweepConfig::default()
            },
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        let first = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(first.scanned(), 3, "the budget is exact, not a floor");
        assert!(first.route(EPHEMERAL_L1).truncated);

        // And the keys it did not reach are still waiting for the next tick.
        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(second.scanned(), 3);
        assert_eq!(sweeper.memo.lock().await.len(), 6);
    }

    /// The bound is derived from the stack so that only a layer reporting more work than the stack
    /// can hold reaches it. A fixed bound also caught a stack that was merely deep, and did it
    /// silently: it dropped the tracking entry, reported the agent archived, and left `open_oplog`'s
    /// suspended worker to make every later tick skip it as resident.
    #[test]
    #[timeout("1m")]
    async fn a_stack_deeper_than_any_fixed_bound_still_drains_fully() {
        let layers = deep_layers(24);
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;

        let sweeper = build(
            &layers,
            manual(),
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        sweeper.sweep_once(&CancellationToken::new()).await;
        let second = sweeper.sweep_once(&CancellationToken::new()).await;

        assert_eq!(second.route(EPHEMERAL_L1).archived, 1);
        assert_eq!(
            second.route(EPHEMERAL_L1).archive_failed,
            0,
            "a stack this deep is a supported configuration, not a miscounting layer"
        );

        // Every layer that can pass entries on has done so. A bound that cut the walk short left
        // them in whichever layer it stopped at, and nothing would have come back for them.
        for (level, archive) in layers.archives.iter().enumerate().rev().skip(1) {
            assert_eq!(
                archive
                    .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                    .await,
                OplogIndex::NONE,
                "layer {level} still holds entries for an agent reported as archived"
            );
        }

        assert_eq!(
            sweeper.max_archive_steps as usize,
            layers.archives.len() + ARCHIVE_STEP_SLACK,
            "the bound follows the stack it was built over"
        );
    }

    #[test]
    async fn a_tick_stops_at_its_page_budget_when_pages_come_back_short() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        for i in 0..10 {
            stranded_ephemeral_oplog(
                &layers,
                &agent(&format!("counter-{i}"), component_id),
                environment_id,
            )
            .await;
        }

        // A budget of eight over a page of four allows two pages. This backend answers with one
        // key however much it is asked for, so the tick never walks its eight and only the page
        // budget can stop it. Without one, a backend that keeps answering short pages holds a
        // tick for as long as it likes.
        let sweeper = build_over(
            Arc::new(FixedPages {
                inner: layers.indexed_storage.clone(),
                keys_per_page: 1,
                duplicate_pages: false,
                cancel_on_scan: std::sync::Mutex::new(None),
                calls: std::sync::atomic::AtomicU64::new(0),
            }),
            &layers,
            OplogSweepConfig {
                enabled: false,
                page_size: 4,
                max_scanned_per_tick: 8,
                ..OplogSweepConfig::default()
            },
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        let report = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(report.scanned(), 2, "one key a page, and two pages allowed");
        assert!(report.route(EPHEMERAL_L1).truncated);
    }

    #[test]
    async fn a_tick_stops_at_its_scan_budget_when_pages_come_back_long() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        for i in 0..10 {
            stranded_ephemeral_oplog(
                &layers,
                &agent(&format!("counter-{i}"), component_id),
                environment_id,
            )
            .await;
        }

        // A budget of six over a page of one leaves room for six pages, so the page budget is not
        // what stops this. The backend answers every ask with four keys and the tick has to count
        // what it was handed, not what it asked for.
        let sweeper = build_over(
            Arc::new(FixedPages {
                inner: layers.indexed_storage.clone(),
                keys_per_page: 4,
                duplicate_pages: false,
                cancel_on_scan: std::sync::Mutex::new(None),
                calls: std::sync::atomic::AtomicU64::new(0),
            }),
            &layers,
            OplogSweepConfig {
                enabled: false,
                page_size: 1,
                max_scanned_per_tick: 6,
                ..OplogSweepConfig::default()
            },
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        let report = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(
            report.scanned(),
            8,
            "two pages of four, the second one carrying past the budget"
        );
        assert!(report.route(EPHEMERAL_L1).truncated);
    }

    #[test]
    #[timeout("1m")]
    async fn a_tick_that_archives_resumes_where_it_left_off() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        let mut resident = HashSet::new();
        for i in 0..6 {
            let agent_id = agent(&format!("counter-{i}"), component_id);
            stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;
            // Two agents that can never be archived, so the tick walks more keys than it removes
            // and something is left behind for a rewind to trip over.
            if i >= 4 {
                resident.insert(agent_id);
            }
        }

        // One page wide enough for the whole namespace, so which keys a page holds does not depend
        // on the storage's iteration order, and an archive budget low enough to stop the tick.
        let sweeper = build(
            &layers,
            OplogSweepConfig {
                enabled: false,
                page_size: 6,
                max_scanned_per_tick: 6,
                max_archives_per_tick: 1,
                ..OplogSweepConfig::default()
            },
            all_shards(),
            environment_id,
            resident,
        );

        // Two ticks to finish the first pass and leave the four candidates quiet.
        sweeper.sweep_once(&CancellationToken::new()).await;
        sweeper.sweep_once(&CancellationToken::new()).await;

        let archiving = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(archiving.scanned(), 6);
        assert_eq!(archiving.archived(), 4);
        assert!(archiving.route(EPHEMERAL_L1).truncated);

        // The tick stopped on its archive budget having walked the whole namespace, so the next one
        // resumes past the last key it saw and finds nothing. A tick that rewound on archiving
        // instead would walk the two it cannot archive again, every time, and on a namespace larger
        // than one tick's budget would never reach the far end at all.
        let next = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(
            next.scanned(),
            0,
            "the tick restarted the namespace instead of resuming"
        );
    }

    #[test]
    async fn a_sweep_without_a_shard_assignment_does_nothing() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;

        let sweeper = build(
            &layers,
            manual(),
            Arc::new(ShardServiceDefault::new()),
            environment_id,
            HashSet::new(),
        );

        let report = sweeper.sweep_once(&CancellationToken::new()).await;
        assert!(report.unassigned);
        assert_eq!(report.scanned(), 0);
        assert_eq!(report.archived(), 0);
    }

    #[test]
    async fn a_sweep_holding_a_zero_shard_assignment_does_nothing() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;

        // Assigning shards before any registration installs `ShardAssignment::default`, which
        // carries shard ids and a shard count of zero. Routing an agent through that count is a
        // division by zero, and a panic here aborts the pod from a background task.
        let shards = Arc::new(ShardServiceDefault::new());
        shards
            .assign_shards(&HashSet::from([ShardId::new(0)]))
            .expect("assignment");
        let sweeper = build(&layers, manual(), shards, environment_id, HashSet::new());

        let report = sweeper.sweep_once(&CancellationToken::new()).await;

        assert!(report.unassigned);
        assert_eq!(report.scanned(), 0);
        assert_eq!(report.archived(), 0);
    }

    #[test]
    async fn a_disabled_sweep_never_ticks() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;
        let sweeper = build(
            &layers,
            OplogSweepConfig {
                enabled: false,
                interval: Duration::from_millis(1),
                ..OplogSweepConfig::default()
            },
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        tokio::time::timeout(
            Duration::from_millis(200),
            sweeper.clone().run(CancellationToken::new()),
        )
        .await
        .expect("a disabled sweep must return instead of ticking");
        assert_eq!(sweeper.memo.lock().await.len(), 0);
    }

    #[test]
    async fn a_stack_with_no_source_layer_never_ticks() {
        let layers = layers();
        let sweeper = OplogSweeper::over_layers(
            OplogSweepConfig {
                enabled: true,
                interval: Duration::from_millis(1),
                ..OplogSweepConfig::default()
            },
            layers.indexed_storage.clone(),
            // Only the bottom layer, which can never be a source.
            &layers.archives[1..],
            all_shards(),
            Arc::new(FixedEnvironment {
                environment_id: EnvironmentId::new(),
                fails: false,
                deleted: false,
                lookups: std::sync::atomic::AtomicU64::new(0),
            }),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                resident: HashSet::new(),
            }),
        );

        assert!(sweeper.routes.is_empty());
        tokio::time::timeout(
            Duration::from_millis(200),
            sweeper.run(CancellationToken::new()),
        )
        .await
        .expect("a sweep with no route must return instead of ticking");
    }

    #[test]
    async fn cancelling_the_token_stops_the_loop() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;
        let sweeper = build(
            &layers,
            OplogSweepConfig {
                enabled: true,
                interval: Duration::from_millis(5),
                ..OplogSweepConfig::default()
            },
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        let token = CancellationToken::new();
        let running = tokio::spawn({
            let sweeper = sweeper.clone();
            let token = token.clone();
            async move { sweeper.run(token).await }
        });

        // Wait for the work rather than for the clock: a fixed sleep asserts how many ticks a
        // loaded machine got through, which is not what this test is about.
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        for _ in 0..400 {
            if layers.archives[1]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await
                != OplogIndex::NONE
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        token.cancel();

        tokio::time::timeout(Duration::from_secs(2), running)
            .await
            .expect("the loop must stop once the token is cancelled")
            .expect("the loop must not panic");

        assert_eq!(
            layers.archives[1]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::from_u64(3),
            "the loop should have archived before it was asked to stop"
        );
    }
}
