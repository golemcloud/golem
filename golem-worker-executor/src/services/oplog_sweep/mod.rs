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

//! Finds oplog layers holding entries for agents that have gone quiet, and archives each agent
//! down through the layers below.
//!
//! `Worker::archive_oplog` does the moving, as it does for `ScheduledAction::ArchiveOplog`, but the
//! sweep waits for each transfer. Where the scheduled action needs a row written on the oplog
//! commit path, the sweep finds its work by scanning the layer itself. An archive step ends in
//! `drop_prefix`, which removes the key, so the scan enumerates work rather than agents.
//!
//! # Agent modes
//!
//! Only ephemeral oplogs are swept. While the sweep is enabled, ephemeral agents register no
//! `ScheduledAction::ArchiveOplog`, so the sweep is what moves an oplog a crashed pod stranded.
//!
//! Durable oplogs are not swept. An archive step interrupted between its append and its
//! `drop_prefix` leaves that prefix to be appended again. Indexed archives reconcile an append
//! error by reading the persisted chunk and accepting it only when its contents match; blob
//! targets use a `put` keyed by the chunk's last index and perform the same content check. A
//! retry therefore drops the source only after the destination is known to contain that prefix.
//!
//! # Failure
//!
//! A failure while inspecting or archiving one agent produces an `archive_failed` outcome. A
//! namespace scan failure truncates that route's tick instead because no agent key was selected.
//! In both cases the authoritative source remains available for a later, backed-off attempt.
//!
//! # Memory
//!
//! An archive step reads a whole source layer into one `Vec`, as `archive_ephemeral_oplog` does on
//! teardown, and a tick runs at most `max_concurrency` of them, since each step waits for its
//! transfer. Each archived agent also leaves the `Worker` that archiving acquires in
//! `ActiveAgents`, where memory-pressure eviction never takes it, until `active_agents.ttl` evicts
//! it.

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
use crate::services::oplog::{ArchiveWait, OplogArchiveService};
use crate::services::scheduler::SchedulerWorkerAccess;
use crate::services::shard::ShardService;
use crate::storage::indexed::{
    IndexedStorage, IndexedStorageLabelledApi, IndexedStorageMetaNamespace,
};
use crate::storage::indexed::{ScanResume, agent_mode_prefix};

/// One archive step: ephemeral entries move out of the layer at `source_level` into the layer
/// below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct RouteId {
    source_level: usize,
}

impl Display for RouteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-l{}",
            agent_mode_prefix(AgentMode::Ephemeral),
            self.source_level
        )
    }
}

/// What the sweep decided about one scanned key. A key whose agent a cancelled tick never started
/// has no outcome, though `scanned` still counts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The key did not parse as `{component_id}:{agent_name}`.
    Unparseable,
    /// Another executor owns this agent's shard.
    NotOwned,
    /// The key lost its entries between the scan and the probe, usually to the teardown drain.
    Empty,
    /// Not shown to be quiet yet: the index moved since the previous pass, or this is the first
    /// sighting.
    Waiting,
    /// `ActiveAgents` holds a worker for the agent, running or suspended.
    Resident,
    /// The agent's component could not be resolved, so its environment is unknown.
    Unaddressable,
    /// The agent's worker declined the archive: it is being deleted, its oplog was retired, its
    /// index moved, it no longer exists, or no archive step recognised its oplog.
    Declined,
    /// The agent was not drained: the archive call failed or the step bound was reached.
    ArchiveFailed,
    /// The agent's entries were moved out of every layer that could pass them on.
    Archived,
}

impl Outcome {
    /// Every outcome, in declaration order, which is also the order [`RouteReport`] counts them in.
    const ALL: [Outcome; 9] = [
        Outcome::Unparseable,
        Outcome::NotOwned,
        Outcome::Empty,
        Outcome::Waiting,
        Outcome::Resident,
        Outcome::Unaddressable,
        Outcome::Declined,
        Outcome::ArchiveFailed,
        Outcome::Archived,
    ];

    /// The `outcome` label on the sweep metrics.
    fn label(self) -> &'static str {
        match self {
            Outcome::Unparseable => "unparseable",
            Outcome::NotOwned => "not_owned",
            Outcome::Empty => "empty",
            Outcome::Waiting => "waiting",
            Outcome::Resident => "resident",
            Outcome::Unaddressable => "unaddressable",
            Outcome::Declined => "declined",
            Outcome::ArchiveFailed => "archive_failed",
            Outcome::Archived => "archived",
        }
    }

    /// Whether deciding this agent cost an archive attempt, which is what both archive budgets are
    /// charged. A declined or failed archive counts. It is attempted again on a later pass, or, when
    /// the attempt left a worker in `ActiveAgents`, once that worker expires.
    fn reached_the_store(self) -> bool {
        matches!(
            self,
            Outcome::Archived | Outcome::Declined | Outcome::ArchiveFailed
        )
    }
}

/// Whether an agent has been quiet long enough to archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Remember this index and reconsider on the next pass.
    Wait,
    /// The index has not moved since an earlier pass, so archive the agent now.
    Archive,
}

/// Slack over the layer count when bounding the archive steps one agent gets in a single tick. A
/// correct stack needs one step per layer, so the bound only stops a layer that miscounts `more`.
const ARCHIVE_STEP_SLACK: usize = 2;

/// Resolved environments held at once, far above the components one executor's shards can hold.
const MAX_CACHED_ENVIRONMENTS: usize = 10_000;

/// How long a component that would not resolve is left alone before it is asked again. Without it
/// every tick repeats two registry calls per failure, and remembering failures forever would strand
/// agents behind a registry that was only briefly unreachable.
const UNRESOLVED_ENVIRONMENT_RETRY: Duration = Duration::from_secs(300);

/// The shortest a tick interval is allowed to be. A misconfigured zero would otherwise spin.
const MIN_INTERVAL: Duration = Duration::from_millis(100);

/// The shortest a tick deadline is allowed to be. A zero would cut every tick before it archives
/// anything.
const MIN_TICK_DURATION: Duration = Duration::from_secs(1);

/// How many intervals to wait before the next tick. Doubles, up to `cap`, while ticks keep hitting
/// their deadline, and resets to `1` once a tick finishes inside it.
fn next_backoff(current: u32, over_deadline: bool, cap: u32) -> u32 {
    if over_deadline {
        current.saturating_mul(2).min(cap.max(1))
    } else {
        1
    }
}

/// Per-route counters. The outcome counters need not sum to `scanned`, since a key a cancelled tick
/// never started has no outcome.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RouteReport {
    scanned: u64,
    /// One counter per [`Outcome`], indexed by its discriminant.
    outcomes: [u64; Outcome::ALL.len()],
    /// Outcomes that spent an archive attempt, by [`Outcome::reached_the_store`], which is what the
    /// archive budgets are charged. Not recorded as an outcome.
    store_visits: u64,
    /// A scan failed, a budget ran out, or the tick was cancelled with agents still unstarted.
    truncated: bool,
}

impl RouteReport {
    fn count(&self, outcome: Outcome) -> u64 {
        self.outcomes[outcome as usize]
    }

    fn record(&self, route: &str, elapsed: std::time::Duration) {
        for outcome in Outcome::ALL {
            record_oplog_sweep_outcome(route, outcome.label(), self.count(outcome));
        }
        record_oplog_sweep_tick(route, elapsed, self.truncated);
    }
}

/// What one call to [`OplogSweeper::sweep_once`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SweepReport {
    routes: Vec<(RouteId, RouteReport)>,
    /// The tick had no usable shard assignment and did nothing.
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
        self.routes
            .iter()
            .map(|(_, r)| r.count(Outcome::Archived))
            .sum()
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
    assignment.contains(&shard_id)
}

/// The quiet gate: the last index is unchanged since a sighting from an *earlier* pass. A sighting
/// from the same pass is not enough, because a backend may return a key twice in one walk, as Redis
/// `SCAN` may, and that would archive an agent without waiting out a pass.
fn assess(remembered: Option<Seen>, current: OplogIndex, pass: u64) -> Verdict {
    match remembered {
        Some(seen) if seen.index == current && seen.pass < pass => Verdict::Archive,
        _ => Verdict::Wait,
    }
}

/// Adds two reports. `truncated` is sticky: a tick that stopped early on any page stopped early.
fn merge(left: RouteReport, right: RouteReport) -> RouteReport {
    let mut outcomes = left.outcomes;
    for (sum, count) in outcomes.iter_mut().zip(right.outcomes) {
        *sum += count;
    }
    RouteReport {
        scanned: left.scanned + right.scanned,
        outcomes,
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
            report.outcomes[outcome as usize] += 1;
            report
        })
}

/// Splits a scanned page, without I/O, into agents worth probing and outcomes for the rest.
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

/// What a tick has left to spend, shared across its routes. Each route takes an equal share of the
/// remainder, so a busy route cannot starve the ones behind it and an idle one leaves its share to
/// them.
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

    /// The next route's share: what is left, split over the routes still to run, rounded up. `None`
    /// once either budget is spent, which stops the tick.
    fn share(&self) -> Option<RouteShare> {
        if self.scans == 0 || self.archives == 0 {
            return None;
        }
        let left = self.routes_left.max(1);
        Some(RouteShare {
            scans: self.scans.div_ceil(left),
            archives: self.archives.div_ceil(left),
        })
    }

    /// Books what a route used and drops it from the split.
    fn spend(&mut self, scanned: u64, archived: u64) {
        self.scans = self.scans.saturating_sub(scanned);
        self.archives = self.archives.saturating_sub(archived);
        self.routes_left = self.routes_left.saturating_sub(1);
    }
}

/// What one route may spend in a tick: keys to walk and archive attempts to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RouteShare {
    scans: u64,
    archives: u64,
}

struct Route {
    id: RouteId,
    namespace: IndexedStorageMetaNamespace,
    source: Arc<dyn OplogArchiveService>,
}

#[derive(Debug, Clone, Copy)]
struct InspectionFailure {
    failures: u32,
    retry_pass: u64,
}

#[derive(Debug, Clone, Copy)]
struct ScanFailure {
    failures: u32,
    retry_at: Instant,
}

fn inspection_backoff_passes(failures: u32) -> u64 {
    1_u64 << failures.saturating_sub(1).min(6)
}

pub struct OplogSweeper {
    config: OplogSweepConfig,
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    routes: Vec<Route>,
    shards: Arc<dyn ShardService>,
    components: Arc<dyn ComponentService>,
    worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync>,
    /// Archive steps one agent gets per visit: the stack depth plus [`ARCHIVE_STEP_SLACK`].
    max_archive_steps: u32,
    /// The index each agent showed and the pass that saw it, per route, which [`assess`] gates on.
    /// A completed pass drops what it did not see, since ephemeral ids are unbounded and a drained
    /// agent never returns. Losing an entry costs a pass of latency, never a stranded oplog.
    memo: Mutex<HashMap<(RouteId, AgentId), Seen>>,
    /// Per-agent inspection failures. Permanent storage errors must not be retried every pass.
    inspection_failures: Mutex<HashMap<(RouteId, AgentId), InspectionFailure>>,
    /// Per-route namespace scan failures. One unavailable route must not hot-loop or delay others.
    scan_failures: Mutex<HashMap<RouteId, ScanFailure>>,
    /// Where each route's scan stopped. Absent means the start of the namespace.
    cursors: Mutex<HashMap<RouteId, ScanResume>>,
    /// Which scan pass each route is on. Bumped when a pass reaches the end of the namespace.
    passes: Mutex<HashMap<RouteId, u64>>,
    /// Environments already resolved, by component, so a cold registry lookup is paid once rather
    /// than every tick. A known entry never goes stale, since a component keeps its environment.
    /// Cleared wholesale past [`MAX_CACHED_ENVIRONMENTS`].
    environments: Mutex<HashMap<ComponentId, ResolvedEnvironment>>,
    /// The route the next tick starts at, so the routes a tick could not afford go first next time.
    route_cursor: Mutex<usize>,
}

impl OplogSweeper {
    /// Derives routes from the archive stack `lib.rs` built. A layer is a source when it can
    /// enumerate its keys and has a layer below it. The primary is not in `archives`, so the
    /// level-0 hop stays with `ScheduledAction::ArchiveOplog`. No I/O; call [`run`](Self::run) to
    /// start ticking.
    pub fn over_layers(
        config: OplogSweepConfig,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        archives: &[Arc<dyn OplogArchiveService>],
        shards: Arc<dyn ShardService>,
        components: Arc<dyn ComponentService>,
        worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync>,
    ) -> Arc<Self> {
        let sources = archives.len().saturating_sub(1);
        let mut routes: Vec<Route> = Vec::new();
        for source in archives.iter().take(sources) {
            // A blob layer cannot enumerate its keys, and answers `None`.
            let Some(namespace @ IndexedStorageMetaNamespace::CompressedOplog { level, .. }) =
                source.scan_namespace(AgentMode::Ephemeral)
            else {
                continue;
            };
            routes.push(Route {
                id: RouteId {
                    source_level: level,
                },
                namespace,
                source: source.clone(),
            });
        }
        // Deepest source first, so a tick never hands entries to a layer it is about to drain.
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
            inspection_failures: Mutex::new(HashMap::new()),
            scan_failures: Mutex::new(HashMap::new()),
            cursors: Mutex::new(HashMap::new()),
            passes: Mutex::new(HashMap::new()),
            environments: Mutex::new(HashMap::new()),
            route_cursor: Mutex::new(0),
        })
    }

    /// Runs ticks until `shutdown` is cancelled. Returns at once when disabled, or when the stack
    /// has no layer to sweep.
    ///
    /// Cancellation, by shutdown or by `max_tick_duration`, is checked only between routes, pages
    /// and agents, so an archive step is never cut between its append below and its `drop_prefix`
    /// above. Spawn this through `Shutdown::spawn`, so shutdown waits up to `SHUTDOWN_GRACE` for
    /// the agent in flight.
    pub async fn run(self: Arc<Self>, shutdown: CancellationToken) {
        if !self.config.enabled || self.routes.is_empty() {
            return;
        }
        let interval = self.config.interval.max(MIN_INTERVAL);
        let tick_deadline = self.config.max_tick_duration.max(MIN_TICK_DURATION);
        let mut backoff: u32 = 1;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tokio::time::sleep(interval.saturating_mul(backoff)) => {}
            }

            // The deadline cancels a child of the shutdown token, so it stops the tick at the same
            // boundaries shutdown does. A deadline that lands between routes drops the unreached
            // routes from the report without marking anything truncated.
            let tick = shutdown.child_token();
            let sweep = self.sweep_once(&tick);
            let deadline = tokio::time::sleep(tick_deadline);
            tokio::pin!(sweep, deadline);
            let report = loop {
                tokio::select! {
                    // Biased, so a sweep finishing as the deadline elapses counts as on time.
                    biased;
                    report = &mut sweep => break report,
                    // Guarded, because a completed `Sleep` stays ready and would spin this select.
                    _ = &mut deadline, if !tick.is_cancelled() => {
                        tick.cancel();
                    }
                }
            };

            // A tick cut by shutdown says nothing about the store, so it does not feed the backoff.
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
                debug!("Oplog sweep tick skipped: this executor holds no live shard assignment");
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

    /// Runs one tick over every route. A storage error ends that route's tick and the next tick
    /// retries, because the work list is the layer itself.
    async fn sweep_once(&self, cancel: &CancellationToken) -> SweepReport {
        // No work without a usable assignment. `ShardAssignment::default` has zero shards, which
        // divides by zero when routing, and after a lapsed lease the shard manager may already have
        // moved the shards to an executor whose running agents the residency probe cannot see. The
        // lease is read once per tick, as the scheduler's poll loop does.
        let now = Instant::now();
        let assignment = self.shards.try_get_current_assignment();
        let Some(assignment) =
            assignment.filter(|it| it.number_of_shards > 0 && it.lease_is_live(now))
        else {
            return SweepReport {
                routes: Vec::new(),
                unassigned: true,
            };
        };

        // Where the last tick stopped.
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
            // Out of time or out of budget: this route and the ones behind it go first next tick.
            let affordable = if cancel.is_cancelled() {
                None
            } else {
                budget.share()
            };
            let Some(share) = affordable else {
                stopped_at = Some(start + offset);
                break;
            };
            let report = self
                .sweep_route(route, &assignment, cancel, share)
                .instrument(info_span!("oplog_sweep", route = %route.id))
                .await;
            budget.spend(report.scanned, report.store_visits);
            routes.push((route.id, report));
        }
        *self.route_cursor.lock().await = stopped_at.unwrap_or(0);

        SweepReport {
            routes,
            unassigned: false,
        }
    }

    /// Walks one route within its share of the tick's budgets. Every key on a page is decided, and
    /// archived if quiet, before the next page is fetched, so a tick that stops early leaves
    /// nothing half-done and one that keeps hitting its deadline still makes progress.
    async fn sweep_route(
        &self,
        route: &Route,
        assignment: &ShardAssignment,
        cancel: &CancellationToken,
        share: RouteShare,
    ) -> RouteReport {
        let started = Instant::now();
        if self.scan_is_backed_off(route.id).await {
            return RouteReport::default();
        }
        let mut resume = self.cursors.lock().await.get(&route.id).cloned();
        let pass = self
            .passes
            .lock()
            .await
            .get(&route.id)
            .copied()
            .unwrap_or(0);
        // Folded page by page, so memory does not grow with the namespace.
        let mut report = RouteReport::default();
        let mut truncated = false;
        let mut exhausted = false;
        let mut scan_failed = false;
        let mut archive_allowance = share.archives;
        let mut walked: u64 = 0;
        let mut pages: u64 = 0;
        // A zero page size would end the pass on an empty page and wipe the tracking table.
        let page_size = self.config.page_size.max(1);
        let scan_budget = share.scans.max(1);
        // Redis can return empty pages while it walks the keyspace, so a key budget alone does not
        // bound round trips.
        let page_budget = scan_budget.div_ceil(page_size).max(1);
        loop {
            if cancel.is_cancelled() {
                truncated = true;
                break;
            }
            // Soft by up to a page, since a page is decided as a unit.
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
            // Ask for no more than the budget allows. A backend may return more, and trimming the
            // page would move the cursor past keys nothing examined.
            let count = page_size.min(allowance);

            let page = self
                .indexed_storage
                .with("oplog_sweep", "scan")
                .scan_stable(route.namespace.clone(), None, resume.clone(), count)
                .await;

            let (next, keys) = match page {
                Ok(page) => page,
                Err(error) => {
                    self.record_scan_failure(route.id).await;
                    crate::metrics::oplog::record_archive_maintenance_failure("sweep_scan");
                    warn!(route = %route.id, error = %error, "Oplog sweep scan failed");
                    scan_failed = true;
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
            archive_allowance = archive_allowance.saturating_sub(page_report.store_visits);
            report = merge(report, page_report);

            // The cursor moves past a cut page, and a cut on the last page still closes the pass.
            // Keys left unstarted stay in the layer for the next pass. Holding the cursor would
            // repeat the page every tick, and within one pass its keys could never become quiet.
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

        // A resume token is a place in the key order, so archiving behind the walk cannot shift it.
        match resume {
            Some(resume) => {
                self.cursors.lock().await.insert(route.id, resume);
            }
            None => {
                self.cursors.lock().await.remove(&route.id);
            }
        }

        if exhausted {
            self.finish_pass(route.id, pass).await;
        }
        if !scan_failed {
            self.scan_failures.lock().await.remove(&route.id);
        }
        self.forget_stale(route.id, assignment).await;

        report.scanned = walked;
        report.truncated = truncated;
        report.record(&route.id.to_string(), started.elapsed());
        report
    }

    /// Decides one agent and, if it is quiet, archives it. `None` means the tick was cancelled
    /// before the agent started. A started agent is finished, so a deadline never discards a read
    /// it paid for.
    ///
    /// Residency and the layer index can go stale, so they are checked last, back to back, right
    /// before the archive they guard, which must not run under a live writer.
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

        // Resolved first, as the only step that can be slow. The layer below addresses objects by
        // environment, which a scanned key does not carry.
        let Some(environment_id) = self.environment_of(agent_id.component_id).await else {
            return Some(Outcome::Unaddressable);
        };
        let owned_agent_id = OwnedAgentId {
            environment_id,
            agent_id: agent_id.clone(),
        };

        // Checked before any storage read: on a busy executor most keys belong to running agents.
        if self.is_resident(&owned_agent_id).await {
            return Some(Outcome::Resident);
        }

        if self
            .inspection_is_backed_off(route.id, &agent_id, pass)
            .await
        {
            return Some(Outcome::Waiting);
        }

        let current = match route
            .source
            .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
            .await
        {
            Ok(current) => current,
            Err(error) => {
                self.record_inspection_failure(route.id, &agent_id, pass)
                    .await;
                crate::metrics::oplog::record_archive_maintenance_failure("sweep_inspection");
                warn!(agent_id = %owned_agent_id, error = %error, "Failed to inspect oplog archive during sweep");
                return Some(Outcome::ArchiveFailed);
            }
        };
        self.clear_inspection_failure(route.id, &agent_id).await;
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
            return Some(Outcome::Waiting);
        }

        // Restamped before archiving: a failed archive leaves the agent in the layer, and on the
        // old stamp `finish_pass` would drop it and restart its gate every pass.
        self.remember(route.id, &agent_id, current, pass).await;
        Some(self.archive_agent(route, owned_agent_id, current).await)
    }

    /// Whether `ActiveAgents` holds a worker for the agent, running or suspended.
    ///
    /// The probe must not refresh the worker's TTL. It runs for every agent on every tick, so a
    /// refreshing probe would keep a suspended worker from ever expiring and its oplog would never
    /// be archived.
    async fn is_resident(&self, owned_agent_id: &OwnedAgentId) -> bool {
        self.worker_access.worker_is_cached(owned_agent_id).await
    }

    /// Moves one agent's entries down through every layer below, up to
    /// [`OplogSweeper::max_archive_steps`] steps.
    ///
    /// Each step waits for its transfer, so `max_concurrency` bounds the transfers in flight.
    /// `max_tick_duration` only keeps further agents from starting, and a started transfer runs to
    /// its end. It drains fully in one visit because archiving leaves the worker in `ActiveAgents`,
    /// and until that worker expires later ticks skip the agent as resident.
    async fn archive_agent(
        &self,
        route: &Route,
        owned_agent_id: OwnedAgentId,
        last_oplog_index: OplogIndex,
    ) -> Outcome {
        let agent_id = &owned_agent_id.agent_id;

        let mut more = true;
        let mut steps = 0;
        while more && steps < self.max_archive_steps {
            // The worker's oplog lifecycle lock is the mutual exclusion, as it is for
            // `ScheduledAction::ArchiveOplog`.
            match self
                .worker_access
                .archive_oplog(&owned_agent_id, last_oplog_index, ArchiveWait::Finished)
                .await
            {
                Ok(Some(remaining)) => more = remaining,
                Ok(None) => {
                    debug!(
                        agent_id = %agent_id,
                        steps,
                        "Oplog sweep archive declined by the agent's worker"
                    );
                    return Outcome::Declined;
                }
                Err(error) => {
                    warn!(
                        agent_id = %agent_id,
                        error = %error,
                        "Oplog sweep could not archive an agent"
                    );
                    return Outcome::ArchiveFailed;
                }
            }
            steps += 1;
        }
        if more {
            // A layer reported more work than the stack can hold; the tracking entry stays.
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
        Outcome::Archived
    }

    /// An agent's environment, resolved through its component: a scanned key carries none, and the
    /// `Create` entry may already have moved down a layer.
    ///
    /// Tries the deployed lookup, then revision zero, which `get_component_metadata` returns even
    /// for a deleted component. A component keeps its environment across revisions, and a stranded
    /// oplog often belongs to a deleted one.
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
                        error = %error,
                        "Oplog sweep could not resolve the component of a stranded oplog"
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
            // A backstop: `finish_pass` drops agents that left the layer. Declining one agent costs
            // it a pass, while clearing the table would wipe every sighting each time a pass
            // refilled it, and nothing would ever be archived.
            warn!(
                tracked = memo.len(),
                agent_id = %agent_id,
                "Oplog sweep tracking table full, not tracking this agent"
            );
            return;
        }
        // A tracked agent still updates when the table is full.
        memo.insert(key, Seen { index, pass });
    }

    async fn inspection_is_backed_off(
        &self,
        route: RouteId,
        agent_id: &AgentId,
        pass: u64,
    ) -> bool {
        self.inspection_failures
            .lock()
            .await
            .get(&(route, agent_id.clone()))
            .is_some_and(|failure| pass < failure.retry_pass)
    }

    async fn scan_is_backed_off(&self, route: RouteId) -> bool {
        self.scan_failures
            .lock()
            .await
            .get(&route)
            .is_some_and(|failure| Instant::now() < failure.retry_at)
    }

    async fn record_scan_failure(&self, route: RouteId) {
        let mut failures = self.scan_failures.lock().await;
        let count = failures
            .get(&route)
            .map_or(1, |failure| failure.failures.saturating_add(1));
        let intervals = 1_u32 << count.saturating_sub(1).min(6);
        let intervals = intervals.min(self.config.max_backoff_intervals.max(1));
        failures.insert(
            route,
            ScanFailure {
                failures: count,
                retry_at: Instant::now() + self.config.interval.saturating_mul(intervals),
            },
        );
    }

    async fn record_inspection_failure(&self, route: RouteId, agent_id: &AgentId, pass: u64) {
        let mut failures = self.inspection_failures.lock().await;
        let key = (route, agent_id.clone());
        if failures.len() >= self.config.max_tracked_agents.max(1) && !failures.contains_key(&key) {
            return;
        }
        let count = failures
            .get(&key)
            .map_or(1, |failure| failure.failures.saturating_add(1));
        failures.insert(
            key,
            InspectionFailure {
                failures: count,
                retry_pass: pass.saturating_add(inspection_backoff_passes(count)),
            },
        );
    }

    async fn clear_inspection_failure(&self, route: RouteId, agent_id: &AgentId) {
        self.inspection_failures
            .lock()
            .await
            .remove(&(route, agent_id.clone()));
    }

    /// Closes a scan pass: entries the pass did not touch belong to agents that have left the
    /// layer, so they are dropped and the route moves on to the next pass.
    async fn finish_pass(&self, route: RouteId, pass: u64) {
        let dropped = {
            let mut memo = self.memo.lock().await;
            let before = memo.len();
            // One pass of grace: a concurrent teardown drain can make a pass miss an agent still in
            // the layer, and dropping it on sight would restart its gate.
            memo.retain(|(memo_route, _), seen| *memo_route != route || seen.pass + 1 >= pass);
            before - memo.len()
        };
        self.inspection_failures
            .lock()
            .await
            .retain(|(failure_route, _), failure| {
                *failure_route != route || failure.retry_pass.saturating_add(1) >= pass
            });
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
        self.clear_inspection_failure(route, agent_id).await;
    }

    /// Drops memo entries for agents this executor no longer owns, so a reshard does not leave them
    /// behind forever.
    async fn forget_stale(&self, route: RouteId, assignment: &ShardAssignment) {
        self.memo
            .lock()
            .await
            .retain(|(memo_route, agent_id), _| *memo_route != route || owns(assignment, agent_id));
        self.inspection_failures
            .lock()
            .await
            .retain(|(failure_route, agent_id), _| {
                *failure_route != route || owns(assignment, agent_id)
            });
    }
}

#[cfg(test)]
mod tests;
