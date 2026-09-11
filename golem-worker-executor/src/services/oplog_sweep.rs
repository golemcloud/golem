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

use std::collections::HashMap;
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

/// What the sweep decided about one scanned key.
///
/// A key the tick decided produces exactly one, which is what makes [`tally`] a total fold over
/// what it decided. One case leaves a walked key without one: the tick was cancelled before the
/// key's agent was started. `scanned` counts it anyway, from the walk rather than from outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The key did not parse as `{component_id}:{agent_name}`.
    Unparseable,
    /// Another executor owns this agent's shard.
    NotOwned,
    /// The layer holds nothing: the key lost its entries between the scan and the probe, usually
    /// to the agent's own teardown drain winning the race.
    Empty,
    /// Not shown to be quiet, so the sweep leaves it alone for now. Either the layer's last index
    /// moved since the previous pass saw it, or there is no sighting to measure against yet, which
    /// is the ordinary state on an agent's first visit.
    Moving,
    /// This executor is running the agent right now.
    Resident,
    /// The agent's component could not be resolved, so the environment its entries belong to is
    /// unknown.
    Unaddressable,
    /// The agent was not drained. Three causes: the oplog would not open, nothing recognised it as
    /// something that can be archived, or the step bound was reached with a layer still reporting
    /// entries below it. The first two move nothing; the third moved what it could and stopped.
    ArchiveFailed,
    /// The agent's entries were moved out of every layer that could pass them on.
    ///
    /// Always a full drain. [`OplogSweeper::archive_agent`] keeps stepping until `try_archive`
    /// reports nothing left below, because `open_oplog` leaves a suspended worker behind that makes
    /// the next tick skip the agent as resident; a partial move would strand the remainder. An
    /// agent the step bound cuts short is reported as [`Outcome::ArchiveFailed`] instead.
    Archived,
}

impl Outcome {
    /// Whether deciding this agent cost an archive attempt.
    ///
    /// Both archive budgets bound exactly that, so both are charged by this one rule rather than
    /// by two hand-written lists that the next variant would be added to only one of.
    ///
    /// [`Outcome::ArchiveFailed`] counts for all three of its causes, including the two that move
    /// nothing: an oplog that would not open, and a layer nothing recognised. Each is an attempt
    /// that will be made again, so charging them is the conservative reading. The third, the step
    /// bound, moved what it could before stopping.
    fn reached_the_store(&self) -> bool {
        match self {
            // Each spent an archive attempt, whether or not it moved anything.
            Outcome::Archived | Outcome::ArchiveFailed => true,
            // All decided before `archive_agent` reaches the store.
            Outcome::Unparseable
            | Outcome::NotOwned
            | Outcome::Empty
            | Outcome::Moving
            | Outcome::Resident
            | Outcome::Unaddressable => false,
        }
    }
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
/// A stack with more than one indexed archive layer gives ephemeral agents an indexed-to-indexed
/// hop with the same append, and that hop is already racy without the sweep: the teardown drain
/// runs after the worker has left `ActiveWorkers`, so a re-invocation can open a second oplog over
/// the same layers and move the same chunk. The sweep is one more thing that can open that second
/// oplog, and `EphemeralOplog::archive` moves the first non-empty layer whichever route found the
/// agent, so no choice of routes avoids it. Raising `indexed_storage_layers` above 2 carries that
/// risk with or without the sweep.
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

/// How long a component that would not resolve is left alone before it is asked again.
///
/// Remembering only successes left the failures being repaid on every tick, two registry calls
/// each, which is the cost the memo exists to remove. Failures are not permanent, though: a
/// registry that was merely unreachable answers later, and a component this never retried would
/// strand its agents for the life of the pod. Untuned, like the other bounds here.
const UNRESOLVED_ENVIRONMENT_RETRY: Duration = Duration::from_secs(300);

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

/// Per-route counters.
///
/// `scanned` counts the keys this route took off the namespace. The outcome counters between it
/// and `store_visits` count one decision each; `store_visits` and `truncated` count neither.
///
/// The outcome counters do not sum to `scanned`: a key whose agent the tick was cancelled before
/// starting has no outcome. `scanned` is set once, from the count the scan loop keeps, and never
/// accumulated from outcomes.
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
    /// Outcomes that spent an archive attempt, by [`Outcome::reached_the_store`]. Not an outcome
    /// of its own and not recorded as one: it is what the archive budgets are charged, counted
    /// where every other outcome is counted so the two cannot drift apart.
    store_visits: u64,
    /// The route did not finish what it set out to do: a scan failed, a budget ran out, or the
    /// tick was cancelled, by its deadline or by shutdown, with agents still unstarted. A tick can
    /// exhaust the namespace and still be truncated, if the cut landed on its last page.
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
/// previous shard owner still writing. Nothing later can catch it, since the two sightings need
/// not even be in the same tick. A pass visits each key once, so an earlier pass is what "quiet
/// since last time" means.
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
        store_visits: left.store_visits + right.store_visits,
        truncated: left.truncated || right.truncated,
    }
}

/// Folds per-key outcomes into a report.
fn tally(outcomes: impl IntoIterator<Item = Outcome>) -> RouteReport {
    outcomes
        .into_iter()
        .fold(RouteReport::default(), |mut report, outcome| {
            report.store_visits += u64::from(outcome.reached_the_store());
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

/// What the registry last said about one component's environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedEnvironment {
    Known(EnvironmentId),
    /// Nothing answered, and when to ask again.
    Unknown {
        retry_after: Instant,
    },
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
/// it. The order routes run in is load-bearing -- deepest source layer first, which is the highest
/// `source_level`, so a tick never hands entries to a layer it is about to drain -- so a tick that
/// cannot afford the whole stack moves
/// its starting point rather than reordering what it does run.
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
    /// tick scanned four keys, and a route that overshot its page did not stop the
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
    /// The index each agent showed on the previous pass, per route, which is what [`assess`] gates
    /// on. Losing an entry costs one extra pass of latency, never a stranded oplog: the work list
    /// comes from storage.
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
    /// Environments already resolved, by component.
    ///
    /// A cold lookup costs a registry call, and for a deleted component two of them. Without this
    /// every tick repaid it for every component it met, and a tick that kept losing its deadline
    /// inside those calls learned nothing for the next one.
    ///
    /// A resolved entry cannot go stale: a component does not move between environments, which is
    /// the same fact that lets `environment_of` fall back to revision zero. A failure can, so it
    /// carries [`UNRESOLVED_ENVIRONMENT_RETRY`]. Cleared wholesale when it outgrows
    /// [`MAX_CACHED_ENVIRONMENTS`] rather than evicted one at a time, because it is a cache and
    /// losing it costs lookups rather than correctness.
    environments: Mutex<HashMap<ComponentId, ResolvedEnvironment>>,
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
        // Deepest source layer first, which is the highest `source_level`, so a tick never hands
        // entries to a layer it is about to drain.
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
            environments: Mutex::new(HashMap::new()),
            route_cursor: Mutex::new(0),
        })
    }

    /// Runs ticks until `shutdown` is cancelled, then returns.
    ///
    /// Cancellation is observed between routes, between scan pages and before each agent, but
    /// never inside one, so shutting down never interrupts an archive step between its append to
    /// the layer below and its drop from the layer above. An agent already under way is finished,
    /// all of its layers, before the loop returns. Spawn this into the executor's `JoinSet` so that shutdown waits
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
            // Out of time and out of budget are the same answer here: this route did not run, and
            // neither will the ones behind it. Deciding both in one place is what keeps the two
            // reasons from needing separate proof that the cursor moves.
            let affordable = if cancel.is_cancelled() {
                None
            } else {
                budget.share()
            };
            let Some((scans, archives)) = affordable else {
                stopped_at = Some(start + offset);
                break;
            };
            let report = self
                .sweep_route(route, &assignment, cancel, scans, archives)
                .instrument(info_span!("oplog_sweep", route = %route.id))
                .await;
            // `store_visits` is the same rule the route's own allowance was charged by, so a tick
            // and a route never disagree about what an archive attempt costs.
            budget.spend(report.scanned, report.store_visits);
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
    ///
    /// One phase. Each page is walked, and every key on it is decided and, if quiet, archived
    /// before the next page is asked for. `scan_stable` resumes by seeking, so a key deleted
    /// behind the walk moves nothing in front of it, and an agent is archived by the same task
    /// that read its index, so a tick that stops early has nothing half-done to remember. The
    /// probe was the expensive half of an earlier two-phase design; committing it immediately is
    /// what lets a tick that keeps hitting its deadline still make progress, without carrying
    /// candidates from one tick to the next.
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
        let pass = self
            .passes
            .lock()
            .await
            .get(&route.id)
            .copied()
            .unwrap_or(0);
        // Folded page by page: holding every outcome would make a tick's memory grow with the
        // namespace, which is what the budgets exist to prevent.
        let mut report = RouteReport::default();
        let mut truncated = false;
        let mut exhausted = false;
        let mut archive_allowance = archive_budget;
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
            // Soft by up to a page: a page is decided as a unit, so this stops at the first page
            // that carries the allowance past its bound rather than splitting one.
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
            let decided: Vec<Option<Outcome>> = stream::iter(candidates)
                .map(|agent_id| self.sweep_agent(route, agent_id, pass, cancel))
                .buffer_unordered(self.config.max_concurrency.max(1))
                .collect()
                .await;
            let unreached = decided.iter().filter(|outcome| outcome.is_none()).count();
            let page_report = tally(settled.into_iter().chain(decided.into_iter().flatten()));
            // The same counter the tick budget is charged, so a route and a tick can never
            // disagree about what an archive attempt costs.
            archive_allowance = archive_allowance.saturating_sub(page_report.store_visits);
            report = merge(report, page_report);

            // The walk moves past a cut page, and a cut on the last page still closes the pass.
            // The keys the cut left unstarted are still in the layer and the next pass finds
            // them, with one pass of grace on their sightings. Holding the cursor or the pass
            // instead would walk the same page every tick, and inside one pass its unchanged
            // keys can never become quiet, so a page too slow for one tick would never drain.
            resume = next;
            exhausted = resume.is_none();
            if unreached > 0 {
                truncated = true;
                break;
            }
            if exhausted {
                break;
            }
        }

        // Stored exactly as it came back. A resume token names a place in the key order, so
        // nothing archived behind the walk can move it.
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

        report.scanned = walked;
        report.truncated = truncated;
        report.record(&route.id.to_string(), started.elapsed());
        report
    }

    /// Decides one agent and, if it is quiet, archives it. `None` means the tick was cancelled
    /// before this agent was started, so nothing about it was read or decided.
    ///
    /// The cancellation check is the first thing and the only one: an agent that has been started
    /// is finished, its probe included, so a deadline never throws away a read it paid for. That
    /// is the whole reason probing and archiving are one task rather than two phases.
    ///
    /// The checks run cheapest first, and the two that can go stale -- residency and the layer
    /// index -- run last and back to back, so that nothing slow sits between them and the
    /// `open_oplog` they guard. `open_oplog` registers a suspended worker that nothing evicts, so
    /// opening one for an agent with nothing left leaks an `ActiveWorkers` entry for the life of
    /// the pod, and opening one under a live writer moves entries out from under it.
    async fn sweep_agent(
        &self,
        route: &Route,
        agent_id: AgentId,
        pass: u64,
        cancel: &CancellationToken,
    ) -> Option<Outcome> {
        if cancel.is_cancelled() {
            return None;
        }

        // The layer below addresses its objects by environment and a scanned key does not carry
        // one. Resolved first because a cold lookup is a registry call, and it is the only thing
        // here that can take long. Memoised per component, and ephemeral agents come in crowds
        // that share one, so almost every call is a hash lookup.
        let Some(environment_id) = self.environment_of(agent_id.component_id).await else {
            return Some(Outcome::Unaddressable);
        };
        let owned_agent_id = OwnedAgentId {
            environment_id,
            agent_id: agent_id.clone(),
        };

        // In memory. On a busy executor most scanned keys belong to agents this pod is running,
        // and probing their index would be one storage read each, every tick, to learn what this
        // answers for free.
        if self.is_resident(&owned_agent_id).await {
            return Some(Outcome::Resident);
        }

        let current = route
            .source
            .get_last_index(&owned_agent_id, route.id.agent_mode)
            .await;
        if current == OplogIndex::NONE {
            self.forget(route.id, &agent_id).await;
            return Some(Outcome::Empty);
        }

        let remembered = self
            .memo
            .lock()
            .await
            .get(&(route.id, agent_id.clone()))
            .copied();
        if assess(remembered, current, pass) == Verdict::Wait {
            self.remember(route.id, &agent_id, current, pass).await;
            return Some(Outcome::Moving);
        }

        // Restamped even though it is about to be archived. A successful archive calls `forget`
        // and the entry goes anyway, but an archive that fails leaves the agent in the layer, and
        // on the old stamp `finish_pass` would drop it: the agent would start its two-pass gate
        // again on every pass without ever getting through it.
        self.remember(route.id, &agent_id, current, pass).await;
        Some(self.archive_agent(route, owned_agent_id).await)
    }

    /// Whether this executor is running the agent right now.
    ///
    /// In memory, so it costs nothing to ask. `ActiveWorkers` keys on the agent id alone, so the
    /// environment on the id is not part of the question.
    async fn is_resident(&self, owned_agent_id: &OwnedAgentId) -> bool {
        self.worker_access
            .active_worker_fingerprint(owned_agent_id)
            .await
            .is_some()
    }

    /// Moves one agent's entries out of this layer, and keeps going until no layer below holds
    /// any, or until [`OplogSweeper::max_archive_steps`] steps have run.
    async fn archive_agent(&self, route: &Route, owned_agent_id: OwnedAgentId) -> Outcome {
        let agent_id = &owned_agent_id.agent_id;

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

        self.forget(route.id, agent_id).await;
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
        match self.environments.lock().await.get(&component_id) {
            Some(ResolvedEnvironment::Known(environment_id)) => return Some(*environment_id),
            Some(ResolvedEnvironment::Unknown { retry_after }) if Instant::now() < *retry_after => {
                return None;
            }
            _ => {}
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
        let entry = match resolved {
            Some(environment_id) => ResolvedEnvironment::Known(environment_id),
            None => ResolvedEnvironment::Unknown {
                retry_after: Instant::now() + UNRESOLVED_ENVIRONMENT_RETRY,
            },
        };
        let mut cache = self.environments.lock().await;
        if cache.len() >= MAX_CACHED_ENVIRONMENTS {
            cache.clear();
        }
        cache.insert(component_id, entry);
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
        MultiLayerOplogService, Oplog, OplogArchive, OplogService, PrimaryOplogService,
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
    /// step: four routes and `max_scanned_per_tick = 1` scanned four keys, and a route that
    /// overshot its page did not stop the routes behind it.
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

        // Overshooting is the same answer. A route stops at the first page that carries it past
        // its bound rather than splitting one, so it can spend more than it was handed.
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
            9,
            "every outcome lands in exactly one counter"
        );
        assert_eq!(
            report.store_visits, 3,
            "and the two that spent an archive attempt are counted again there"
        );
        assert_eq!(
            report.scanned, 0,
            "`tally` does not touch `scanned`: a walked key is charged once, by the route that \
             walked it, from the count the scan loop already keeps"
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
            store_visits: 11,
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
            store_visits: 21,
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
        assert_eq!(merged.store_visits, 32);
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

    /// The same stack as [`layers`], but with every layer reading and writing through the returned
    /// wrapper, so a test can count what the sweep asks the store for.
    ///
    /// [`layers`] builds its archives directly over the in-memory storage, so a wrapper handed to
    /// `build_over` only ever sees the sweeper's own scans; `OplogArchiveService::get_last_index`
    /// goes straight past it.
    fn counting_layers() -> (Layers, Arc<FixedPages>) {
        let inner = Arc::new(InMemoryIndexedStorage::new());
        let storage = Arc::new(FixedPages {
            inner: inner.clone(),
            keys_per_page: 10,
            duplicate_pages: false,
            cancel_on_scan: std::sync::Mutex::new(None),
            cancel_on_last_id: std::sync::Mutex::new(None),
            last_ids: std::sync::atomic::AtomicU64::new(0),
            calls: std::sync::atomic::AtomicU64::new(0),
        });
        let blob_storage = Arc::new(InMemoryBlobStorage::new());
        let compressed: Arc<dyn OplogArchiveService> = Arc::new(
            CompressedOplogArchiveService::new(storage.clone(), 1, RetryConfig::default()),
        );
        let blob: Arc<dyn OplogArchiveService> =
            Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 0));
        let layers = Layers {
            oplog_service: Arc::new(MultiLayerOplogService::new(
                Arc::new(futures::executor::block_on(PrimaryOplogService::new(
                    storage.clone(),
                    blob_storage.clone(),
                    100,
                    100,
                    1024,
                    RetryConfig::default(),
                ))),
                nev![compressed.clone(), blob.clone()],
                1000,
                1000,
            )),
            archives: vec![compressed, blob],
            indexed_storage: inner,
        };
        (layers, storage)
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
        /// Cancelled once a page has been served, then disarmed: a deadline that lands before any
        /// agent on the page has been started.
        cancel_on_scan: std::sync::Mutex<Option<CancellationToken>>,
        /// Cancelled while the next index read is in flight, then disarmed: a deadline that lands
        /// inside an agent's probe, which is where a slow store puts it. See
        /// `OplogSweepConfig::max_tick_duration`.
        cancel_on_last_id: std::sync::Mutex<Option<CancellationToken>>,
        /// `OplogArchiveService::get_last_index` bottoms out here, so this counts the index reads
        /// the sweep makes.
        last_ids: std::sync::atomic::AtomicU64,
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
            self.last_ids
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(token) = self.cancel_on_last_id.lock().unwrap().take() {
                token.cancel();
            }
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
        /// Started running on the first lookup, then disarmed. The environment lookup is the one
        /// slow step before an agent's residency and index are checked, so this is the seam that
        /// proves those checks come after it.
        resident_on_lookup: std::sync::Mutex<Option<(ResidentSet, AgentId)>>,
        /// Appends an entry to a layer on the first lookup, then disarms. Same seam as
        /// `resident_on_lookup`, for the layer index.
        grow_on_lookup: std::sync::Mutex<Option<LayerGrowth>>,
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
            if let Some((resident, agent_id)) = self.resident_on_lookup.lock().unwrap().take() {
                resident.lock().unwrap().insert(agent_id);
            }
            let grow = self.grow_on_lookup.lock().unwrap().take();
            if let Some((archive, owned_agent_id, at)) = grow {
                archive
                    .open(&owned_agent_id, AgentMode::Ephemeral)
                    .await
                    .append(vec![(at, OplogEntry::suspend())])
                    .await;
            }
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
        /// Shared and mutable, so a test can start running an agent between the tick that probed
        /// it and the tick that comes to archive it.
        resident: Arc<std::sync::Mutex<HashSet<AgentId>>>,
        /// Agents whose oplog will not open, which is one of the three ways `archive_agent`
        /// reports [`Outcome::ArchiveFailed`]. It is the one that never reaches the store, and it
        /// is chosen here because it is the only one of the three a test double can force; what
        /// the test pins is the budget rule, which charges all three alike.
        refuse_open: HashSet<AgentId>,
    }

    type ResidentSet = Arc<std::sync::Mutex<HashSet<AgentId>>>;

    /// A layer, an agent in it, and the index to append at.
    type LayerGrowth = (Arc<dyn OplogArchiveService>, OwnedAgentId, OplogIndex);

    fn resident_set(agents: HashSet<AgentId>) -> ResidentSet {
        Arc::new(std::sync::Mutex::new(agents))
    }

    #[async_trait]
    impl SchedulerWorkerAccess for DirectAccess {
        async fn active_worker_fingerprint(
            &self,
            owned_agent_id: &OwnedAgentId,
        ) -> Option<AgentFingerprint> {
            self.resident
                .lock()
                .unwrap()
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
            if self.refuse_open.contains(&owned_agent_id.agent_id) {
                return Err(WorkerExecutorError::runtime("oplog will not open"));
            }
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
                resident_on_lookup: std::sync::Mutex::new(None),
                grow_on_lookup: std::sync::Mutex::new(None),
            }),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                refuse_open: HashSet::new(),
                resident: resident_set(resident),
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

    /// Regression, in shape. A store slow enough that every tick lost its deadline while probing
    /// used to leave a sweep that walked its namespace forever and archived nothing, and that
    /// store is the one condition the deadline exists for. An agent is archived by the task that
    /// read its index, so a deadline that lands during the read cannot throw the read away.
    #[test]
    #[timeout("1m")]
    async fn a_tick_cut_during_a_probe_still_archives_the_agent_it_probed() {
        // Read through the wrapper, so the index read is where the seam fires.
        let (layers, storage) = counting_layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        let sweeper = build_over(
            storage.clone(),
            &layers,
            manual(),
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        // Unarmed: the first tick only records the index.
        let first = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(first.route(EPHEMERAL_L1).moving, 1);

        // Armed. The gate is open, and the deadline lands while the agent's index is being read.
        let cut = CancellationToken::new();
        *storage.cancel_on_last_id.lock().unwrap() = Some(cut.clone());
        let second = sweeper.sweep_once(&cut).await;
        assert!(cut.is_cancelled(), "the deadline landed inside the probe");
        assert_eq!(
            second.archived(),
            1,
            "a read the tick paid for is acted on, not thrown away"
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

    /// The finding that started this: 128 candidates, one-second index reads and a 30-second
    /// deadline meant probing alone outlasted every tick, and the sweep never archived. Here every
    /// tick is cut on its first index read and runs one agent at a time, and the layer still
    /// drains at two ticks per agent, because the agent whose read the cut landed in is finished
    /// and the rest wait in the layer for the next pass.
    #[test]
    #[timeout("1m")]
    async fn a_sweep_cut_on_every_tick_still_drains_the_layer() {
        let (layers, storage) = counting_layers();
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        let agents: Vec<AgentId> = (0..4)
            .map(|i| agent(&format!("counter-{i}"), component_id))
            .collect();
        for agent_id in &agents {
            stranded_ephemeral_oplog(&layers, agent_id, environment_id).await;
        }
        let sweeper = build_over(
            storage.clone(),
            &layers,
            OplogSweepConfig {
                max_concurrency: 1,
                ..manual()
            },
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        let mut ticks = 0;
        let mut archived = 0;
        while archived < agents.len() {
            assert!(
                ticks < 2 * agents.len() + 2,
                "{} of {} agents archived after {ticks} ticks, each cut on its first read",
                archived,
                agents.len()
            );
            let cut = CancellationToken::new();
            *storage.cancel_on_last_id.lock().unwrap() = Some(cut.clone());
            let report = sweeper.sweep_once(&cut).await;
            assert!(cut.is_cancelled(), "tick {ticks} was not cut");
            archived += report.archived() as usize;
            ticks += 1;
        }
        for agent_id in &agents {
            let owned_agent_id = OwnedAgentId::new(environment_id, agent_id);
            assert_eq!(
                layers.archives[0]
                    .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                    .await,
                OplogIndex::NONE,
                "{agent_id} is still in the layer"
            );
            assert_eq!(
                layers.archives[1]
                    .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                    .await,
                OplogIndex::from_u64(3)
            );
        }
    }

    /// A tick cut before it starts a page's agents decides nothing about them, says so, and leaves
    /// them for the next pass, sightings intact.
    #[test]
    #[timeout("1m")]
    async fn a_tick_cut_before_a_page_s_agents_leaves_them_for_the_next_pass() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;
        let storage = pages_of(&layers);
        let sweeper = build_over(
            storage.clone(),
            &layers,
            manual(),
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        let first = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(first.route(EPHEMERAL_L1).moving, 1);

        // The deadline lands as soon as the page is served, before any agent on it is started.
        let cut = CancellationToken::new();
        *storage.cancel_on_scan.lock().unwrap() = Some(cut.clone());
        let second = sweeper.sweep_once(&cut).await;
        assert_eq!(
            second.route(EPHEMERAL_L1),
            RouteReport {
                scanned: 1,
                truncated: true,
                ..RouteReport::default()
            },
            "walked one key, decided nothing about it, and reports the tick as cut"
        );

        let third = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(
            third.archived(),
            1,
            "the sighting from before the cut pass still opens the gate"
        );
    }

    /// Regression: `environment_of` used to pay a registry lookup per component per tick, two of
    /// them for a deleted component. A tick that kept losing its deadline inside those calls
    /// learned nothing for the next one. Remembering the answer is what bounds that.
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
            resident_on_lookup: std::sync::Mutex::new(None),
            grow_on_lookup: std::sync::Mutex::new(None),
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
                refuse_open: HashSet::new(),
                resident: resident_set(HashSet::new()),
            }),
        );

        // The first tick resolves the component both agents share, once, before it reads either
        // index.
        sweeper.sweep_once(&CancellationToken::new()).await;
        let after_first_resolve = components.lookups.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            after_first_resolve, 2,
            "one deployed miss and one pinned hit, for the component both agents share"
        );

        // The second archives both agents and asks nothing further.
        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(second.archived(), 2);
        assert_eq!(
            components.lookups.load(std::sync::atomic::Ordering::SeqCst),
            after_first_resolve
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

    /// Remembering only the successes left the failures repaid on every tick, two registry calls
    /// each, which is the cost the memo exists to remove.
    #[test]
    #[timeout("1m")]
    async fn a_component_that_will_not_resolve_is_not_asked_again_every_tick() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        let components = Arc::new(FixedEnvironment {
            environment_id,
            fails: true,
            deleted: false,
            lookups: std::sync::atomic::AtomicU64::new(0),
            resident_on_lookup: std::sync::Mutex::new(None),
            grow_on_lookup: std::sync::Mutex::new(None),
        });
        stranded_ephemeral_oplog(&layers, &agent("counter-1", component_id), environment_id).await;

        let sweeper = OplogSweeper::over_layers(
            manual(),
            layers.indexed_storage.clone(),
            &layers.archives,
            all_shards(),
            components.clone(),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                refuse_open: HashSet::new(),
                resident: resident_set(HashSet::new()),
            }),
        );

        sweeper.sweep_once(&CancellationToken::new()).await;
        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(
            second.route(EPHEMERAL_L1).unaddressable,
            1,
            "the agent is still reported rather than skipped silently"
        );
        let after_failing = components.lookups.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            after_failing, 2,
            "the deployed lookup and the pinned one, both of which failed"
        );

        // The agent stays in the layer, so every later tick meets it again.
        for _ in 0..3 {
            sweeper.sweep_once(&CancellationToken::new()).await;
        }
        assert_eq!(
            components.lookups.load(std::sync::atomic::Ordering::SeqCst),
            after_failing,
            "and the failure is remembered, not repaid once per tick"
        );
    }

    fn sweeper_over(
        layers: &Layers,
        config: OplogSweepConfig,
        storage: Arc<dyn IndexedStorage + Send + Sync>,
        components: Arc<FixedEnvironment>,
        resident: Arc<std::sync::Mutex<HashSet<AgentId>>>,
    ) -> Arc<OplogSweeper> {
        OplogSweeper::over_layers(
            config,
            storage,
            &layers.archives,
            all_shards(),
            components,
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                refuse_open: HashSet::new(),
                resident,
            }),
        )
    }

    fn pages_of(layers: &Layers) -> Arc<FixedPages> {
        Arc::new(FixedPages {
            inner: layers.indexed_storage.clone(),
            keys_per_page: 10,
            duplicate_pages: false,
            cancel_on_scan: std::sync::Mutex::new(None),
            cancel_on_last_id: std::sync::Mutex::new(None),
            last_ids: std::sync::atomic::AtomicU64::new(0),
            calls: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Remembering a failure forever would strand every agent on a component whose registry was
    /// only briefly unreachable, which is why the entry carries a window rather than being final.
    #[test]
    #[timeout("1m")]
    async fn a_failed_lookup_is_asked_again_once_its_window_has_passed() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        let components = Arc::new(FixedEnvironment {
            environment_id,
            fails: true,
            deleted: false,
            lookups: std::sync::atomic::AtomicU64::new(0),
            resident_on_lookup: std::sync::Mutex::new(None),
            grow_on_lookup: std::sync::Mutex::new(None),
        });
        stranded_ephemeral_oplog(&layers, &agent("counter-1", component_id), environment_id).await;
        let sweeper = sweeper_over(
            &layers,
            manual(),
            layers.indexed_storage.clone(),
            components.clone(),
            resident_set(HashSet::new()),
        );

        sweeper.sweep_once(&CancellationToken::new()).await;
        sweeper.sweep_once(&CancellationToken::new()).await;
        let while_remembered = components.lookups.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(while_remembered, 2);
        assert!(
            matches!(
                sweeper.environments.lock().await.get(&component_id),
                Some(ResolvedEnvironment::Unknown { .. })
            ),
            "the failure has to be remembered before there is a window to expire; without this \
             the rewind below iterates an empty map and proves nothing"
        );

        // Wind the window into the past rather than waiting it out.
        {
            let mut cache = sweeper.environments.lock().await;
            for entry in cache.values_mut() {
                *entry = ResolvedEnvironment::Unknown {
                    retry_after: Instant::now() - Duration::from_secs(1),
                };
            }
        }

        sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(
            components.lookups.load(std::sync::atomic::Ordering::SeqCst),
            while_remembered + 2,
            "the deployed lookup and the pinned one are both tried again"
        );
    }

    /// Same seam, for the layer index: it is read after the lookup, so a write that lands during
    /// the lookup is seen and nothing is moved out from under the writer.
    #[test]
    #[timeout("1m")]
    async fn an_agent_whose_layer_grows_during_its_environment_lookup_is_left_alone() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        let components = Arc::new(FixedEnvironment {
            environment_id,
            fails: false,
            deleted: false,
            lookups: std::sync::atomic::AtomicU64::new(0),
            resident_on_lookup: std::sync::Mutex::new(None),
            grow_on_lookup: std::sync::Mutex::new(None),
        });
        let sweeper = sweeper_over(
            &layers,
            manual(),
            layers.indexed_storage.clone(),
            components.clone(),
            resident_set(HashSet::new()),
        );

        // Records the index, so the next tick's probe opens the gate.
        sweeper.sweep_once(&CancellationToken::new()).await;

        // Forgotten, so the next tick's lookup is a registry call, and something writes to the
        // layer while that call is in flight.
        sweeper.environments.lock().await.clear();
        *components.grow_on_lookup.lock().unwrap() = Some((
            layers.archives[0].clone(),
            owned_agent_id.clone(),
            OplogIndex::from_u64(4),
        ));

        let after = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(after.archived(), 0);
        assert_eq!(
            after.route(EPHEMERAL_L1).moving,
            1,
            "the index moved before it was read, so the agent is not quiet"
        );
        assert_eq!(
            layers.archives[0]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::from_u64(4),
            "and nothing was moved out from under the writer"
        );
    }

    /// The environment lookup is the one slow step on the way to `open_oplog`, so the residency
    /// check comes after it: an agent that starts running during the lookup is seen.
    #[test]
    #[timeout("1m")]
    async fn an_agent_that_starts_running_during_its_environment_lookup_is_left_alone() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        let storage = pages_of(&layers);
        let resident = resident_set(HashSet::new());
        let components = Arc::new(FixedEnvironment {
            environment_id,
            fails: false,
            deleted: false,
            lookups: std::sync::atomic::AtomicU64::new(0),
            resident_on_lookup: std::sync::Mutex::new(None),
            grow_on_lookup: std::sync::Mutex::new(None),
        });
        let sweeper = sweeper_over(
            &layers,
            manual(),
            storage.clone(),
            components.clone(),
            resident.clone(),
        );

        sweeper.sweep_once(&CancellationToken::new()).await;

        // Forgotten, so the next tick's lookup is a registry call rather than a cache hit, and the
        // agent starts running while that call is in flight.
        sweeper.environments.lock().await.clear();
        *components.resident_on_lookup.lock().unwrap() = Some((resident.clone(), agent_id.clone()));
        let after = sweeper.sweep_once(&CancellationToken::new()).await;

        assert_eq!(after.archived(), 0);
        assert_eq!(after.route(EPHEMERAL_L1).resident, 1);
        assert_eq!(
            layers.archives[0]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::from_u64(3),
            "the entries stay where the running agent expects them"
        );
    }

    /// The one rule both archive budgets are charged by. Only these two spend an archive attempt;
    /// charging the rest would let a batch of agents that turned out to be resident, or whose
    /// layer had emptied, exhaust the allowance without a single layer having been moved.
    #[test]
    fn only_an_archive_attempt_costs_archive_budget() {
        for outcome in [Outcome::Archived, Outcome::ArchiveFailed] {
            assert!(outcome.reached_the_store(), "{outcome:?}");
        }
        for outcome in [
            Outcome::Unparseable,
            Outcome::NotOwned,
            Outcome::Empty,
            Outcome::Moving,
            Outcome::Resident,
            Outcome::Unaddressable,
        ] {
            assert!(!outcome.reached_the_store(), "{outcome:?}");
        }
    }

    /// Puts an agent's entries straight into one archive layer.
    ///
    /// A sweep drains an agent through every layer in one visit, so nothing settles in a deeper
    /// compressed level at rest and a stack's second route has nothing to scan unless it is seeded
    /// like this. The real route into that state is an agent that outgrew `entry_count_limit`.
    async fn seed_layer(
        archive: &Arc<dyn OplogArchiveService>,
        agent_id: &AgentId,
        environment_id: EnvironmentId,
    ) {
        let owned_agent_id = OwnedAgentId::new(environment_id, agent_id);
        let layer = archive.open(&owned_agent_id, AgentMode::Ephemeral).await;
        layer
            .append(vec![
                (OplogIndex::INITIAL, create_entry(agent_id, environment_id)),
                (OplogIndex::from_u64(2), OplogEntry::exited()),
            ])
            .await;
    }

    /// What a route spends has to come off the tick's budget, or a stack with several source
    /// layers does as much work per route as the config allows for the whole tick. Only observable
    /// with two routes that both have something to scan, since the charge is what the next route's
    /// share is computed from.
    #[test]
    #[timeout("1m")]
    async fn what_one_route_spends_is_taken_off_the_next_route_s_share() {
        let layers = deep_layers(2);
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        seed_layer(
            &layers.archives[1],
            &agent("deep", component_id),
            environment_id,
        )
        .await;
        seed_layer(
            &layers.archives[0],
            &agent("shallow", component_id),
            environment_id,
        )
        .await;

        let sweeper = build(
            &layers,
            OplogSweepConfig {
                // One key for the whole tick, so the first route to find something spends it all.
                max_scanned_per_tick: 1,
                ..manual()
            },
            all_shards(),
            environment_id,
            HashSet::new(),
        );
        assert_eq!(sweeper.routes.len(), 2);
        let deepest = sweeper.routes[0].id;
        let shallowest = sweeper.routes[1].id;

        let first = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(first.scanned(), 1, "one key, as configured");
        assert_eq!(
            first.routes.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![deepest],
            "the second route could not be afforded once the first had spent the budget"
        );
        assert_eq!(
            *sweeper.route_cursor.lock().await,
            1,
            "so the next tick starts with it"
        );

        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(
            second.routes.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![shallowest]
        );
        assert_eq!(second.scanned(), 1);
        assert_eq!(*sweeper.route_cursor.lock().await, 0);
    }

    /// The archive half of the tick budget, which is not the same as the archived count: a failed
    /// archive spent an archive attempt and will be attempted again, so it is charged like
    /// a successful one. Charging the tick only for successes lets a stack with several source
    /// layers do `max_archives_per_tick` archives per route instead of per tick.
    #[test]
    #[timeout("1m")]
    async fn a_failed_archive_costs_the_tick_what_a_successful_one_costs() {
        let layers = deep_layers(2);
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        let deep_agent = agent("deep", component_id);
        seed_layer(&layers.archives[1], &deep_agent, environment_id).await;
        seed_layer(
            &layers.archives[0],
            &agent("shallow", component_id),
            environment_id,
        )
        .await;

        let sweeper = OplogSweeper::over_layers(
            OplogSweepConfig {
                // One archive for the whole tick, shared between the two routes.
                max_archives_per_tick: 1,
                ..manual()
            },
            layers.indexed_storage.clone(),
            &layers.archives,
            all_shards(),
            Arc::new(FixedEnvironment {
                environment_id,
                fails: false,
                deleted: false,
                lookups: std::sync::atomic::AtomicU64::new(0),
                resident_on_lookup: std::sync::Mutex::new(None),
                grow_on_lookup: std::sync::Mutex::new(None),
            }),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                // So the deeper route's agent reaches `archive_agent` and comes back failed,
                // rather than being settled before it gets there.
                refuse_open: HashSet::from([deep_agent.clone()]),
                resident: resident_set(HashSet::new()),
            }),
        );
        let deepest = sweeper.routes[0].id;

        // Both routes only record an index on the first tick, so neither spends an archive.
        let first = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(
            first.routes.len(),
            2,
            "no archive was spent, so both routes could still be afforded"
        );

        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(
            second.route(deepest).archive_failed,
            1,
            "the deeper route spent an archive attempt and moved nothing"
        );
        assert_eq!(second.archived(), 0, "and archived nothing at all");
        assert_eq!(
            second.routes.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![deepest],
            "yet the tick's archive budget is spent, so the second route did not run"
        );
    }

    /// The environment memo is cleared wholesale rather than evicted one entry at a time, so the
    /// bound has to be a bound rather than the point at which the cache stops working.
    #[test]
    #[timeout("1m")]
    async fn a_full_environment_cache_is_cleared_and_keeps_working() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let sweeper = build(
            &layers,
            manual(),
            all_shards(),
            environment_id,
            HashSet::new(),
        );

        {
            let mut cache = sweeper.environments.lock().await;
            for _ in 0..MAX_CACHED_ENVIRONMENTS {
                cache.insert(
                    ComponentId::new(),
                    ResolvedEnvironment::Known(EnvironmentId::new()),
                );
            }
        }

        let fresh = ComponentId::new();
        assert_eq!(sweeper.environment_of(fresh).await, Some(environment_id));

        let cache = sweeper.environments.lock().await;
        assert_eq!(
            cache.len(),
            1,
            "the full cache was dropped rather than left to grow past its bound"
        );
        assert!(
            matches!(cache.get(&fresh), Some(ResolvedEnvironment::Known(_))),
            "and the answer that overflowed it is the one kept"
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
            cancel_on_last_id: std::sync::Mutex::new(None),
            last_ids: std::sync::atomic::AtomicU64::new(0),
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
        let (layers, storage) = counting_layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;
        let reads_before = storage.last_ids.load(std::sync::atomic::Ordering::SeqCst);

        let sweeper = sweeper_over(
            &layers,
            manual(),
            storage.clone(),
            Arc::new(FixedEnvironment {
                environment_id,
                fails: true,
                deleted: false,
                lookups: std::sync::atomic::AtomicU64::new(0),
                resident_on_lookup: std::sync::Mutex::new(None),
                grow_on_lookup: std::sync::Mutex::new(None),
            }),
            resident_set(HashSet::new()),
        );

        sweeper.sweep_once(&CancellationToken::new()).await;
        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(second.route(EPHEMERAL_L1).unaddressable, 1);
        assert_eq!(second.archived(), 0);
        assert_eq!(
            storage.last_ids.load(std::sync::atomic::Ordering::SeqCst),
            reads_before,
            "and its index is never read: nothing could act on the answer"
        );
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
                resident_on_lookup: std::sync::Mutex::new(None),
                grow_on_lookup: std::sync::Mutex::new(None),
            }),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                refuse_open: HashSet::new(),
                resident: resident_set(HashSet::new()),
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
                cancel_on_last_id: std::sync::Mutex::new(None),
                last_ids: std::sync::atomic::AtomicU64::new(0),
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

        // An agent that passes the quiet gate and then fails to archive, here because its oplog
        // will not open, stays in the layer. It has to keep the sighting that got it through the
        // gate: if the pass that archives it does not restamp the entry, `finish_pass` eventually
        // drops it and the agent starts the two-tick gate over, on every pass, forever.
        let sweeper = OplogSweeper::over_layers(
            manual(),
            layers.indexed_storage.clone(),
            &layers.archives,
            all_shards(),
            Arc::new(FixedEnvironment {
                environment_id,
                fails: false,
                deleted: false,
                lookups: std::sync::atomic::AtomicU64::new(0),
                resident_on_lookup: std::sync::Mutex::new(None),
                grow_on_lookup: std::sync::Mutex::new(None),
            }),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                refuse_open: HashSet::from([agent_id.clone()]),
                resident: resident_set(HashSet::new()),
            }),
        );

        // The first tick only records the index; every tick after it should reach the archive and
        // fail there, never fall back to being a fresh sighting.
        sweeper.sweep_once(&CancellationToken::new()).await;
        for tick in 2..=5 {
            let report = sweeper.sweep_once(&CancellationToken::new()).await;
            assert_eq!(
                report.route(EPHEMERAL_L1).archive_failed,
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

    /// A layer that reports more work below it than the stack can hold. `drop_prefix` does not
    /// drop, so the ephemeral archive step finds the same first non-empty layer every time and
    /// says there is more; the layer below swallows the repeated append, since a real indexed
    /// layer would refuse the duplicate ids.
    #[derive(Debug)]
    struct Miscounting {
        inner: Arc<dyn OplogArchiveService>,
        keeps_entries: bool,
        appends: Arc<std::sync::atomic::AtomicU64>,
    }

    #[async_trait]
    impl OplogArchiveService for Miscounting {
        async fn open(
            &self,
            owned_agent_id: &OwnedAgentId,
            agent_mode: AgentMode,
        ) -> Arc<dyn OplogArchive + Send + Sync> {
            Arc::new(MiscountingArchive {
                inner: self.inner.open(owned_agent_id, agent_mode).await,
                keeps_entries: self.keeps_entries,
                appends: self.appends.clone(),
            })
        }

        async fn delete(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) {
            self.inner.delete(owned_agent_id, agent_mode).await
        }

        async fn read(
            &self,
            owned_agent_id: &OwnedAgentId,
            agent_mode: AgentMode,
            idx: OplogIndex,
            n: u64,
        ) -> BTreeMap<OplogIndex, OplogEntry> {
            self.inner.read(owned_agent_id, agent_mode, idx, n).await
        }

        async fn exists(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) -> bool {
            self.inner.exists(owned_agent_id, agent_mode).await
        }

        async fn scan_for_component(
            &self,
            environment_id: &EnvironmentId,
            component_id: &ComponentId,
            modes: Option<AgentMode>,
            cursor: golem_common::model::ScanCursor,
            count: u64,
        ) -> Result<(golem_common::model::ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError>
        {
            self.inner
                .scan_for_component(environment_id, component_id, modes, cursor, count)
                .await
        }

        async fn get_last_index(
            &self,
            owned_agent_id: &OwnedAgentId,
            agent_mode: AgentMode,
        ) -> OplogIndex {
            self.inner.get_last_index(owned_agent_id, agent_mode).await
        }

        fn scan_namespace(&self, agent_mode: AgentMode) -> Option<IndexedStorageMetaNamespace> {
            self.inner.scan_namespace(agent_mode)
        }
    }

    #[derive(Debug)]
    struct MiscountingArchive {
        inner: Arc<dyn OplogArchive + Send + Sync>,
        keeps_entries: bool,
        appends: Arc<std::sync::atomic::AtomicU64>,
    }

    #[async_trait]
    impl OplogArchive for MiscountingArchive {
        async fn read(&self, idx: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
            self.inner.read(idx, n).await
        }

        async fn append(&self, chunk: Vec<(OplogIndex, OplogEntry)>) -> u64 {
            self.appends
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.keeps_entries {
                self.inner.append(chunk).await
            } else {
                0
            }
        }

        async fn current_oplog_index(&self) -> OplogIndex {
            self.inner.current_oplog_index().await
        }

        async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
            if self.keeps_entries {
                0
            } else {
                self.inner.drop_prefix(last_dropped_id).await
            }
        }

        async fn length(&self) -> u64 {
            self.inner.length().await
        }

        async fn get_last_index(&self) -> OplogIndex {
            self.inner.get_last_index().await
        }
    }

    /// The step bound exists for exactly one layer: one that keeps reporting more work below it.
    /// Without the bound `archive_agent` would step forever; with it, the agent is reported as
    /// not archived and its tracking entry stays, rather than being reported archived with its
    /// entries still in place.
    #[test]
    #[timeout("1m")]
    async fn a_layer_that_keeps_reporting_more_work_is_stopped_at_the_step_bound() {
        let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
        let blob_storage = Arc::new(InMemoryBlobStorage::new());
        let appends = Arc::new(std::sync::atomic::AtomicU64::new(0));
        // Level 1 never empties, so every step moves it again; level 2 swallows what it is
        // handed, so the repeat lands nowhere; blob is the bottom that makes level 2 a source.
        let sticky: Arc<dyn OplogArchiveService> = Arc::new(Miscounting {
            inner: Arc::new(CompressedOplogArchiveService::new(
                indexed_storage.clone(),
                1,
                RetryConfig::default(),
            )),
            keeps_entries: true,
            appends: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        });
        let sink: Arc<dyn OplogArchiveService> = Arc::new(Miscounting {
            inner: Arc::new(CompressedOplogArchiveService::new(
                indexed_storage.clone(),
                2,
                RetryConfig::default(),
            )),
            keeps_entries: false,
            appends: appends.clone(),
        });
        let blob: Arc<dyn OplogArchiveService> =
            Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 0));
        let layers = Layers {
            oplog_service: Arc::new(MultiLayerOplogService::new(
                Arc::new(futures::executor::block_on(PrimaryOplogService::new(
                    indexed_storage.clone(),
                    blob_storage.clone(),
                    100,
                    100,
                    1024,
                    RetryConfig::default(),
                ))),
                nev![sticky.clone(), sink.clone(), blob.clone()],
                1000,
                1000,
            )),
            archives: vec![sticky, sink, blob],
            indexed_storage,
        };
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
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

        assert_eq!(second.route(EPHEMERAL_L1).archive_failed, 1);
        assert_eq!(second.archived(), 0, "moved nothing to completion");
        assert_eq!(
            appends.load(std::sync::atomic::Ordering::SeqCst),
            u64::from(sweeper.max_archive_steps),
            "one step per allowed step, and then it stopped"
        );
        assert_eq!(
            sweeper.memo.lock().await.len(),
            1,
            "and the agent keeps its tracking entry, since it was not archived"
        );
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
                cancel_on_last_id: std::sync::Mutex::new(None),
                last_ids: std::sync::atomic::AtomicU64::new(0),
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
                cancel_on_last_id: std::sync::Mutex::new(None),
                last_ids: std::sync::atomic::AtomicU64::new(0),
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
                resident_on_lookup: std::sync::Mutex::new(None),
                grow_on_lookup: std::sync::Mutex::new(None),
            }),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                refuse_open: HashSet::new(),
                resident: resident_set(HashSet::new()),
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
