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
//! `drop_prefix` leaves that prefix to be appended again, and in an indexed layer the repeated
//! `INSERT` hits a unique violation, which `retry_storage_op` turns into a panic. A blob target, which the ephemeral hop
//! uses in the default stack, appends with a `put` keyed by the chunk's last index, so a repeat is
//! harmless. A stack with more than one indexed archive layer gives ephemeral agents the same
//! hazard, which a re-invocation racing the teardown drain can hit with or without the sweep.
//!
//! # Failure
//!
//! A non-transient indexed-storage error panics through `retry_storage_op`, and under
//! `panic = "abort"` that takes the process down, as it does for every other oplog operation.
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
                    warn!(route = %route.id, error = %error, "Oplog sweep scan failed");
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

        let current = route
            .source
            .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
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
        BlobOplogArchiveService, CommitLevel, CompressedOplogArchiveService, EphemeralOplog,
        MultiLayerOplog, MultiLayerOplogService, OplogArchive, OplogService, PrimaryOplogService,
    };
    use crate::services::shard::ShardServiceDefault;
    use crate::storage::indexed::memory::InMemoryIndexedStorage;
    use crate::storage::indexed::{IndexedStorageError, IndexedStorageNamespace};
    use async_trait::async_trait;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::{OwnerKind, Principal};
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
    use golem_common::model::component_metadata::ComponentMetadata;
    use golem_common::model::environment::EnvironmentName;
    use golem_common::model::oplog::OplogEntry;
    use golem_common::model::worker::AgentConfigEntryDto;
    use golem_common::model::{
        AgentFingerprint, AgentInvocation, AgentMetadata, AgentStatusRecord, IdempotencyKey,
        RetryConfig, ShardEpoch, ShardLeaseRevision, Timestamp,
    };
    use golem_common::read_only_lock;
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_service_base::model::component::Component;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use nonempty_collections::nev;
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::sync::RwLock;
    use std::time::Duration;
    use test_r::{test, timeout};
    use uuid::Uuid;

    const EPHEMERAL_L1: RouteId = RouteId { source_level: 1 };

    fn agent(name: &str, component_id: ComponentId) -> AgentId {
        AgentId {
            component_id,
            agent_id: name.to_string(),
        }
    }

    fn create_entry(agent_id: &AgentId, environment_id: EnvironmentId) -> OplogEntry {
        OplogEntry::create(Box::new(golem_common::model::oplog::CreateParameters {
            agent_id: agent_id.clone(),
            owner_kind: OwnerKind::ComponentAgent,
            agent_mode: AgentMode::Ephemeral,
            component_revision: ComponentRevision::new(1).unwrap(),
            env: Vec::new(),
            environment_id,
            created_by: AccountId::new(),
            parent: None,
            component_size: 100,
            initial_total_linear_memory_size: 100,
            initial_active_plugins: HashSet::new(),
            local_agent_config: Vec::new(),
            original_phantom_id: None,
            instance_id: Uuid::new_v4(),
        }))
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

        let mine = ShardAssignment::unexpiring(4, [shard_id]);
        let theirs = ShardAssignment::unexpiring(4, []);

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
        assert_eq!(assess(seen(second, 0), second, 1), Verdict::Archive);
    }

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
            Verdict::Archive,
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
            Some(RouteShare {
                scans: 50,
                archives: 5
            }),
            "an even split, not the whole of it"
        );

        // A route that finds nothing leaves its share to the one behind it rather than wasting it.
        budget.spend(0, 0);
        assert_eq!(
            budget.share(),
            Some(RouteShare {
                scans: 100,
                archives: 10
            })
        );

        let mut budget = TickBudget::new(&config, 2);
        budget.spend(50, 5);
        assert_eq!(
            budget.share(),
            Some(RouteShare {
                scans: 50,
                archives: 5
            }),
            "and a route that spends its share does not"
        );
    }

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
            Some(RouteShare {
                scans: 1,
                archives: 1
            }),
            "the first route gets the one key there is to spend"
        );
        budget.spend(1, 1);
        assert_eq!(
            budget.share(),
            None,
            "and there is nothing left for a second"
        );

        // Overshooting a share spends the budget too.
        let mut budget = TickBudget::new(&config, 4);
        budget.spend(10, 10);
        assert_eq!(budget.share(), None);

        // Either budget alone stops the tick.
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

    #[test]
    fn one_tick_inside_its_deadline_returns_the_sweep_to_full_pace() {
        assert_eq!(next_backoff(8, false, 8), 1);
        assert_eq!(next_backoff(1, false, 8), 1);
    }

    #[test]
    fn a_zero_cap_is_read_as_one_rather_than_disabling_the_wait() {
        assert_eq!(next_backoff(1, true, 0), 1);
        assert_eq!(next_backoff(4, true, 0), 1);
    }

    #[test]
    fn a_route_names_its_mode_and_source_level() {
        assert_eq!(EPHEMERAL_L1.to_string(), "ephemeral-l1");
        assert_eq!(RouteId { source_level: 2 }.to_string(), "ephemeral-l2");
    }

    #[test]
    fn only_ephemeral_layers_are_routed() {
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
            "the ephemeral route is not optional"
        );
        let other: Vec<&IndexedStorageMetaNamespace> = sweeper
            .routes
            .iter()
            .map(|route| &route.namespace)
            .filter(|namespace| {
                !matches!(
                    namespace,
                    IndexedStorageMetaNamespace::CompressedOplog {
                        agent_mode: AgentMode::Ephemeral,
                        ..
                    }
                )
            })
            .collect();
        assert!(
            other.is_empty(),
            "routed a namespace the archive step cannot survive: {other:?}"
        );
    }

    #[test]
    fn tally_counts_every_outcome_once() {
        let report = tally([
            Outcome::Unparseable,
            Outcome::NotOwned,
            Outcome::Empty,
            Outcome::Waiting,
            Outcome::Resident,
            Outcome::Unaddressable,
            Outcome::Declined,
            Outcome::ArchiveFailed,
            Outcome::Archived,
            Outcome::Archived,
        ]);

        assert_eq!(report.count(Outcome::Archived), 2);
        assert_eq!(
            report.outcomes.iter().sum::<u64>(),
            10,
            "every outcome lands in exactly one counter"
        );
        assert_eq!(
            report.store_visits, 4,
            "and the ones that spent an archive attempt are counted again there"
        );
        assert_eq!(
            report.scanned, 0,
            "`tally` does not touch `scanned`: a walked key is charged once, by the route that \
             walked it, from the count the scan loop already keeps"
        );
    }

    #[test]
    fn outcomes_are_counted_under_their_own_index() {
        for (index, outcome) in Outcome::ALL.into_iter().enumerate() {
            assert_eq!(outcome as usize, index, "{outcome:?}");
        }
    }

    #[test]
    fn merge_adds_every_field() {
        // Distinct non-zero values, so a wrongly added field cannot match by coincidence.
        let left = RouteReport {
            scanned: 2,
            outcomes: [3, 4, 5, 6, 7, 8, 9, 10, 11],
            store_visits: 12,
            truncated: false,
        };
        let right = RouteReport {
            scanned: 13,
            outcomes: [14, 15, 16, 17, 18, 19, 20, 21, 22],
            store_visits: 23,
            truncated: true,
        };

        let merged = merge(left, right);
        assert_eq!(merged.scanned, 15);
        assert_eq!(merged.outcomes, [17, 19, 21, 23, 25, 27, 29, 31, 33]);
        assert_eq!(merged.store_visits, 35);
        assert!(merged.truncated, "truncation is sticky");
    }

    #[test]
    fn triage_settles_what_it_can_without_touching_storage() {
        let component_id = ComponentId::new();
        let mine = agent("mine", component_id);
        let my_shard = ShardId::from_routing_hash(ShardId::hash_agent_id(&mine), 4);
        // Found rather than guessed: with four shards a guessed name often lands on the same shard.
        let theirs = (0..)
            .map(|i| agent(&format!("theirs-{i}"), component_id))
            .find(|candidate| {
                ShardId::from_routing_hash(ShardId::hash_agent_id(candidate), 4) != my_shard
            })
            .expect("no agent name maps to another shard");
        let assignment = ShardAssignment::unexpiring(4, [my_shard]);

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

    /// The same stack as [`layers`], with every layer going through the returned wrapper, so a test
    /// can count index reads as well as the sweeper's scans.
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
                // High enough that the size trigger never fires.
                1000,
                1000,
            )),
            archives: vec![compressed, blob],
            indexed_storage,
        }
    }

    /// Hands back a fixed number of keys per page whatever it was asked for, as Redis may, and
    /// forwards everything else.
    #[derive(Debug)]
    struct FixedPages {
        inner: Arc<InMemoryIndexedStorage>,
        keys_per_page: u64,
        /// Serves the whole namespace twice per walk, ignoring the resume token, as Redis `SCAN`
        /// may.
        duplicate_pages: bool,
        /// Cancelled once a page has been served, then disarmed.
        cancel_on_scan: std::sync::Mutex<Option<CancellationToken>>,
        /// Cancelled while the next index read is in flight, then disarmed.
        cancel_on_last_id: std::sync::Mutex<Option<CancellationToken>>,
        /// Counts index reads, which `OplogArchiveService::get_last_index` bottoms out in.
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
            owner_kind: OwnerKind::ComponentAgent,
            env: vec![],
            environment_id,
            created_by: AccountId::new(),
            created_by_email: AccountEmail::new("test@golem"),
            config: Vec::new(),
            created_at: Timestamp::now_utc(),
            parent: None,
            last_known_status: AgentStatusRecord::default(),
            original_phantom_id: None,
            fingerprint: AgentFingerprint::new(),
            agent_mode: AgentMode::Ephemeral,
        }
    }

    fn status_lock() -> read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord> {
        read_only_lock::arc_swap::ReadOnlyView::new(Arc::new(arc_swap::ArcSwap::from_pointee(
            AgentStatusRecord::default(),
        )))
    }

    fn execution_lock() -> read_only_lock::std::ReadOnlyLock<ExecutionStatus> {
        read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(ExecutionStatus::Suspended {
            agent_mode: AgentMode::Ephemeral,
            timestamp: Timestamp::now_utc(),
        })))
    }

    /// Resolves every component to one environment.
    struct FixedEnvironment {
        environment_id: EnvironmentId,
        /// Answers nothing at all, deployed lookup or pinned.
        fails: bool,
        /// Answers only a pinned revision, as for a deleted component.
        deleted: bool,
        /// Registry lookups made.
        lookups: std::sync::atomic::AtomicU64,
        /// Starts running the agent on the first lookup, then disarms.
        resident_on_lookup: std::sync::Mutex<Option<(ResidentSet, AgentId)>>,
        /// Appends to a layer on the first lookup, then disarms.
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
                    .append(&[(at, OplogEntry::suspend())])
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
                account_email: AccountEmail::new("test@golem"),
                application_name: ApplicationName("sweep-test-app".to_string()),
                environment_name: EnvironmentName::try_from("sweep-test-env").unwrap(),
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

    /// Archives without building a `Worker`: it opens the oplog under the agent's lifecycle guard
    /// and runs the same blocking steps as `Worker::archive_oplog` with `ArchiveWait::Finished`.
    struct DirectAccess {
        oplog_service: Arc<dyn OplogService>,
        /// Shared, so a test can start running an agent between ticks.
        resident: Arc<std::sync::Mutex<HashSet<AgentId>>>,
        /// Agents whose archive call fails, the one cause of [`Outcome::ArchiveFailed`] a test
        /// double can force.
        refuse_archive: HashSet<AgentId>,
        /// Agents whose worker declines the archive.
        decline_archive: HashSet<AgentId>,
    }

    type ResidentSet = Arc<std::sync::Mutex<HashSet<AgentId>>>;

    /// A layer, an agent in it, and the index to append at.
    type LayerGrowth = (Arc<dyn OplogArchiveService>, OwnedAgentId, OplogIndex);

    fn resident_set(agents: HashSet<AgentId>) -> ResidentSet {
        Arc::new(std::sync::Mutex::new(agents))
    }

    #[async_trait]
    impl SchedulerWorkerAccess for DirectAccess {
        async fn expire_durable_stream_session(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _target_agent_fingerprint: AgentFingerprint,
            _public_session_id: String,
            _session_key: IdempotencyKey,
            _expected_deadline_millis: u64,
        ) -> Result<(), WorkerExecutorError> {
            unreachable!("the sweep never expires durable stream sessions")
        }

        async fn active_worker_fingerprint(
            &self,
            _owned_agent_id: &OwnedAgentId,
        ) -> Option<AgentFingerprint> {
            unreachable!("the sweep probes residency without refreshing the worker's TTL")
        }

        async fn worker_is_cached(&self, owned_agent_id: &OwnedAgentId) -> bool {
            self.resident
                .lock()
                .unwrap()
                .contains(&owned_agent_id.agent_id)
        }

        async fn activate_worker(
            &self,
            _owned_agent_id: &OwnedAgentId,
        ) -> Result<(), WorkerExecutorError> {
            unreachable!("the sweep never activates an agent")
        }

        async fn archive_oplog(
            &self,
            owned_agent_id: &OwnedAgentId,
            _last_oplog_index: OplogIndex,
            wait: ArchiveWait,
        ) -> Result<Option<bool>, WorkerExecutorError> {
            assert_eq!(
                wait,
                ArchiveWait::Finished,
                "the sweep waits for every transfer"
            );
            if self.refuse_archive.contains(&owned_agent_id.agent_id) {
                return Err(WorkerExecutorError::runtime("archive refused"));
            }
            if self.decline_archive.contains(&owned_agent_id.agent_id) {
                return Ok(None);
            }
            let mut lifecycle = self
                .oplog_service
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await;
            let oplog = self
                .oplog_service
                .open(
                    &mut lifecycle,
                    owned_agent_id,
                    AgentMode::Ephemeral,
                    None,
                    metadata(&owned_agent_id.agent_id, owned_agent_id.environment_id),
                    status_lock(),
                    execution_lock(),
                )
                .await;
            Ok(match MultiLayerOplog::try_archive_blocking(&oplog).await {
                Some(more) => Some(more),
                None => EphemeralOplog::try_archive_blocking(&oplog).await,
            })
        }

        async fn enqueue_invocation(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _invocation: AgentInvocation,
            _worker_env: Option<Vec<(String, String)>>,
            _worker_agent_config: Vec<AgentConfigEntryDto>,
            _component_revision: Option<ComponentRevision>,
            _worker_parent: Option<AgentId>,
            _worker_creation_principal: Principal,
        ) -> Result<(), WorkerExecutorError> {
            unreachable!("the sweep never enqueues invocations")
        }

        async fn enqueue_exact_existing(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _target_worker_fingerprint: AgentFingerprint,
            _invocation: AgentInvocation,
        ) -> Result<bool, WorkerExecutorError> {
            unreachable!("the sweep never enqueues invocations")
        }

        async fn enqueue_ephemeral_external_tool(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _invocation: AgentInvocation,
            _component_revision: ComponentRevision,
        ) -> Result<(), WorkerExecutorError> {
            unreachable!("the sweep never enqueues invocations")
        }
    }

    fn all_shards() -> Arc<ShardServiceDefault> {
        let shard_service = Arc::new(ShardServiceDefault::new());
        shard_service.register(
            1,
            &HashMap::from([(ShardId::new(0), ShardEpoch(0))]),
            None,
            ShardLeaseRevision(0),
        );
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
                refuse_archive: HashSet::new(),
                decline_archive: HashSet::new(),
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
                &mut layers.oplog_service.lock_lifecycle(agent_id).await,
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

        // Ephemeral entries start in the first lower layer, and the layer below is empty.
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
        assert_eq!(first.route(EPHEMERAL_L1).count(Outcome::Waiting), 1);
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
            .read_exact(
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
        assert_eq!(first.route(EPHEMERAL_L1).count(Outcome::Waiting), 1);

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

    /// Every tick is cut on its first index read and runs one agent at a time, and the layer still
    /// drains at two ticks per agent.
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

    #[test]
    #[timeout("1m")]
    async fn a_tick_cut_before_a_pages_agents_leaves_them_for_the_next_pass() {
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
        assert_eq!(first.route(EPHEMERAL_L1).count(Outcome::Waiting), 1);

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

    #[test]
    #[timeout("1m")]
    async fn an_environment_is_resolved_once_and_not_once_per_tick() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        // Deleted, so a cold lookup costs two calls: deployed misses, then the pinned one answers.
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
                refuse_archive: HashSet::new(),
                decline_archive: HashSet::new(),
                resident: resident_set(HashSet::new()),
            }),
        );

        // The first tick resolves the shared component once, before reading either index.
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
                refuse_archive: HashSet::new(),
                decline_archive: HashSet::new(),
                resident: resident_set(HashSet::new()),
            }),
        );

        sweeper.sweep_once(&CancellationToken::new()).await;
        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(
            second.route(EPHEMERAL_L1).count(Outcome::Unaddressable),
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
                refuse_archive: HashSet::new(),
                decline_archive: HashSet::new(),
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

    /// The index is read after the environment lookup, so a write during the lookup is seen.
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

        // Forgotten, so the next lookup is a registry call, and the layer grows during it.
        sweeper.environments.lock().await.clear();
        *components.grow_on_lookup.lock().unwrap() = Some((
            layers.archives[0].clone(),
            owned_agent_id.clone(),
            OplogIndex::from_u64(4),
        ));

        let after = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(after.archived(), 0);
        assert_eq!(
            after.route(EPHEMERAL_L1).count(Outcome::Waiting),
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

    /// Residency is checked after the environment lookup, so an agent started during it is seen.
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

        // Forgotten, so the next lookup is a registry call, and the agent starts during it.
        sweeper.environments.lock().await.clear();
        *components.resident_on_lookup.lock().unwrap() = Some((resident.clone(), agent_id.clone()));
        let after = sweeper.sweep_once(&CancellationToken::new()).await;

        assert_eq!(after.archived(), 0);
        assert_eq!(after.route(EPHEMERAL_L1).count(Outcome::Resident), 1);
        assert_eq!(
            layers.archives[0]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::from_u64(3),
            "the entries stay where the running agent expects them"
        );
    }

    /// Charging the other outcomes would let resident or emptied agents exhaust the allowance
    /// without moving anything.
    #[test]
    fn only_an_archive_attempt_costs_archive_budget() {
        for outcome in [Outcome::Archived, Outcome::Declined, Outcome::ArchiveFailed] {
            assert!(outcome.reached_the_store(), "{outcome:?}");
        }
        for outcome in [
            Outcome::Unparseable,
            Outcome::NotOwned,
            Outcome::Empty,
            Outcome::Waiting,
            Outcome::Resident,
            Outcome::Unaddressable,
        ] {
            assert!(!outcome.reached_the_store(), "{outcome:?}");
        }
    }

    /// Puts an agent's entries straight into one archive layer. A sweep drains through every layer
    /// in one visit, so a deeper level has nothing to scan unless seeded.
    async fn seed_layer(
        archive: &Arc<dyn OplogArchiveService>,
        agent_id: &AgentId,
        environment_id: EnvironmentId,
    ) {
        let owned_agent_id = OwnedAgentId::new(environment_id, agent_id);
        let layer = archive.open(&owned_agent_id, AgentMode::Ephemeral).await;
        layer
            .append(&[
                (OplogIndex::INITIAL, create_entry(agent_id, environment_id)),
                (OplogIndex::from_u64(2), OplogEntry::exited()),
            ])
            .await;
    }

    /// Without this charge, a stack with several source layers would do a whole tick's work per
    /// route.
    #[test]
    #[timeout("1m")]
    async fn what_one_route_spends_comes_off_the_next_routes_share() {
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

    /// A failed archive will be attempted again, so it spends the tick's archive budget like a
    /// successful one.
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
                // So the deeper agent reaches `archive_agent` and fails there.
                refuse_archive: HashSet::from([deep_agent.clone()]),
                decline_archive: HashSet::new(),
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
            second.route(deepest).count(Outcome::ArchiveFailed),
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

        // The deepest source still runs first within a tick.
        let third = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(
            third.routes.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![sweeper.routes[0].id, sweeper.routes[1].id]
        );
    }

    #[test]
    #[timeout("1m")]
    async fn an_agent_invoked_a_second_time_is_still_archived() {
        // After the first archive step moves `Create` down a layer, a second invocation leaves
        // entries above `OplogIndex::INITIAL` with no `Create`, which is normal for an agent
        // invoked twice.
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
                &mut layers.oplog_service.lock_lifecycle(&agent_id).await,
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
            .read_source(
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
        assert_eq!(second.route(EPHEMERAL_L1).count(Outcome::Unaddressable), 0);
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
        assert_eq!(second.route(EPHEMERAL_L1).count(Outcome::Unaddressable), 1);
        assert_eq!(second.archived(), 0);
        assert_eq!(
            storage.last_ids.load(std::sync::atomic::Ordering::SeqCst),
            reads_before,
            "and its index is never read: nothing could act on the answer"
        );
    }

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
                refuse_archive: HashSet::new(),
                decline_archive: HashSet::new(),
                resident: resident_set(HashSet::new()),
            }),
        );

        sweeper.sweep_once(&CancellationToken::new()).await;
        let second = sweeper.sweep_once(&CancellationToken::new()).await;

        assert_eq!(second.route(EPHEMERAL_L1).count(Outcome::Unaddressable), 0);
        assert_eq!(second.archived(), 1);
        assert_eq!(
            layers.archives[0]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::NONE
        );
    }

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

        // One tick, two pages, the same key on both: two sightings from one pass open nothing.
        let first = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(first.route(EPHEMERAL_L1).count(Outcome::Waiting), 2);
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

        // A failed archive keeps the sighting that passed the gate, or `finish_pass` would drop it
        // and the agent would restart its gate every pass.
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
                refuse_archive: HashSet::from([agent_id.clone()]),
                decline_archive: HashSet::new(),
                resident: resident_set(HashSet::new()),
            }),
        );

        // The first tick records the index; every later tick reaches the archive and fails there.
        sweeper.sweep_once(&CancellationToken::new()).await;
        for tick in 2..=5 {
            let report = sweeper.sweep_once(&CancellationToken::new()).await;
            assert_eq!(
                report.route(EPHEMERAL_L1).count(Outcome::ArchiveFailed),
                1,
                "tick {tick} should still be trying to archive"
            );
            assert_eq!(
                report.route(EPHEMERAL_L1).count(Outcome::Waiting),
                0,
                "tick {tick} lost the sighting and restarted the quiet gate"
            );
        }
    }

    #[test]
    async fn a_declined_archive_moves_nothing_and_is_asked_again() {
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
                fails: false,
                deleted: false,
                lookups: std::sync::atomic::AtomicU64::new(0),
                resident_on_lookup: std::sync::Mutex::new(None),
                grow_on_lookup: std::sync::Mutex::new(None),
            }),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                refuse_archive: HashSet::new(),
                decline_archive: HashSet::from([agent_id.clone()]),
                resident: resident_set(HashSet::new()),
            }),
        );

        // No worker stays cached, as when the agent no longer exists, so the next pass asks again.
        sweeper.sweep_once(&CancellationToken::new()).await;
        for tick in 2..=3 {
            let report = sweeper.sweep_once(&CancellationToken::new()).await;
            assert_eq!(
                report.route(EPHEMERAL_L1).count(Outcome::Declined),
                1,
                "tick {tick} should still be asking the worker"
            );
            assert_eq!(report.archived(), 0);
        }
        assert_ne!(
            layers.archives[0]
                .get_last_index(
                    &OwnedAgentId::new(environment_id, &agent_id),
                    AgentMode::Ephemeral
                )
                .await,
            OplogIndex::NONE,
            "the entries stay in the layer"
        );
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

        // Residency is decided in memory, so it lands on the first tick.
        let first = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(first.route(EPHEMERAL_L1).count(Outcome::Resident), 1);
        assert_eq!(first.archived(), 0);

        // And nothing about a running agent is remembered, because it was never a candidate.
        assert!(sweeper.memo.lock().await.is_empty());

        let second = sweeper.sweep_once(&CancellationToken::new()).await;
        assert_eq!(second.route(EPHEMERAL_L1).count(Outcome::Resident), 1);
        assert_eq!(second.archived(), 0);
    }

    /// Appends to an agent's oplog so its last index keeps moving.
    async fn keep_moving(layers: &Layers, agent_id: &AgentId, environment_id: EnvironmentId) {
        let oplog = layers
            .oplog_service
            .open(
                &mut layers.oplog_service.lock_lifecycle(agent_id).await,
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
        // An agent seen once and then drained by its own teardown never returns, so its entry has
        // to go when a pass completes.
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
        shards.register(4, &HashMap::new(), None, ShardLeaseRevision(0));
        let sweeper = build(&layers, manual(), shards, environment_id, HashSet::new());

        sweeper.sweep_once(&CancellationToken::new()).await;
        let second = sweeper.sweep_once(&CancellationToken::new()).await;

        assert_eq!(second.route(EPHEMERAL_L1).count(Outcome::NotOwned), 1);
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
            .assign_shards(4, &HashMap::new(), ShardLeaseRevision(1))
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

        // One key per scan and per tick. Nothing is archived, so the cursor stays valid.
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

        // One key per page, so a tick pages four times before archiving. Every agent it walked is
        // archived, not just those on its last page.
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
        assert_eq!(first.route(EPHEMERAL_L1).count(Outcome::Waiting), 4);

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
        // first agent's sighting, and neither would ever get a second.
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

    /// A layer that reports more work below it than the stack can hold: `drop_prefix` does not
    /// drop, and the layer below swallows the repeated appends a real indexed layer would refuse.
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

        async fn open_fresh(
            &self,
            owned_agent_id: &OwnedAgentId,
            agent_mode: AgentMode,
        ) -> Arc<dyn OplogArchive + Send + Sync> {
            Arc::new(MiscountingArchive {
                inner: self.inner.open_fresh(owned_agent_id, agent_mode).await,
                keeps_entries: self.keeps_entries,
                appends: self.appends.clone(),
            })
        }

        async fn delete(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) {
            self.inner.delete(owned_agent_id, agent_mode).await
        }

        async fn read_source(
            &self,
            owned_agent_id: &OwnedAgentId,
            agent_mode: AgentMode,
            idx: OplogIndex,
            n: u64,
        ) -> BTreeMap<OplogIndex, OplogEntry> {
            self.inner
                .read_source(owned_agent_id, agent_mode, idx, n)
                .await
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
        async fn read_source(&self, idx: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
            self.inner.read_source(idx, n).await
        }

        async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> u64 {
            self.appends
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.keeps_entries {
                self.inner.append(chunk).await
            } else {
                0
            }
        }

        async fn verify_persisted(&self, entries: &[(OplogIndex, OplogEntry)]) {
            if self.keeps_entries {
                self.inner.verify_persisted(entries).await
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

        assert_eq!(second.route(EPHEMERAL_L1).count(Outcome::ArchiveFailed), 1);
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

        assert_eq!(second.route(EPHEMERAL_L1).count(Outcome::Archived), 1);
        assert_eq!(
            second.route(EPHEMERAL_L1).count(Outcome::ArchiveFailed),
            0,
            "a stack this deep is a supported configuration, not a miscounting layer"
        );

        // Every layer that can pass entries on has done so.
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

        // A budget of eight over pages of four allows two pages, and this backend answers one key
        // per ask, so only the page budget stops the tick.
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

        // Six pages of one fit the budget, but the backend answers four keys per ask, so the tick
        // has to count what it was handed.
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
            // Two agents that can never be archived, left behind for a rewind to trip over.
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

        // The tick stopped on its archive budget after walking the whole namespace, so the next one
        // resumes past the last key instead of rewinding onto the two it cannot archive.
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

        // A registration can install shard ids with a shard count of zero, and routing an agent
        // through it would divide by zero.
        let shards = Arc::new(ShardServiceDefault::new());
        shards.register(
            0,
            &HashMap::from([(ShardId::new(0), ShardEpoch(0))]),
            None,
            ShardLeaseRevision(0),
        );
        let sweeper = build(&layers, manual(), shards, environment_id, HashSet::new());

        let report = sweeper.sweep_once(&CancellationToken::new()).await;

        assert!(report.unassigned);
        assert_eq!(report.scanned(), 0);
        assert_eq!(report.archived(), 0);
    }

    #[test]
    async fn a_sweep_holding_a_lapsed_lease_does_nothing() {
        let layers = layers();
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id, environment_id).await;
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

        // Expires as it is granted, so the lease has lapsed by the time a tick reads it.
        let shards = Arc::new(ShardServiceDefault::new());
        shards.register(
            1,
            &HashMap::from([(ShardId::new(0), ShardEpoch(0))]),
            Some(Instant::now()),
            ShardLeaseRevision(0),
        );
        let sweeper = build(&layers, manual(), shards, environment_id, HashSet::new());

        // Two ticks, which is what a quiet agent takes to be archived under a live lease.
        for _ in 0..2 {
            let report = sweeper.sweep_once(&CancellationToken::new()).await;
            assert!(report.unassigned);
            assert_eq!(report.scanned(), 0);
        }
        assert_eq!(
            layers.archives[0]
                .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
                .await,
            OplogIndex::from_u64(3),
            "the entries stay in the layer"
        );
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
                refuse_archive: HashSet::new(),
                decline_archive: HashSet::new(),
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

        // Wait for the work rather than the clock: a fixed sleep would assert how many ticks a
        // loaded machine managed.
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
