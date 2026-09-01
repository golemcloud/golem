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

//! Finds oplog layers that hold entries for agents which have gone quiet, and runs one archive
//! step against each.
//!
//! This is the work list, not the mover. `MultiLayerOplog::try_archive_blocking` and
//! `EphemeralOplog::try_archive_blocking` already move a prefix down one layer, handle both the
//! primary hop and the lower hops, and report whether another layer still holds entries. The
//! sweeper decides *which* agents to run that against.
//!
//! `ScheduledAction::ArchiveOplog` answers the same question from a row written on the oplog
//! commit path, one synchronous scheduler-storage write per invocation, held under
//! `update_state_lock`. The sweeper answers it from a paginated scan of the layer itself, so the
//! write disappears and the scheduler table stops accumulating a row per invocation.
//!
//! The layer being scanned is self-cleaning: an archive step ends in `drop_prefix`, which removes
//! the key. So the scan enumerates work rather than agents, and a sweep that finds nothing costs
//! one storage round trip.
//!
//! # What a tick costs
//!
//! The scan pages, the fan-out is capped by `max_concurrency`, and a tick stops after
//! `max_archives_per_tick` archive steps, keeping its cursor so the next tick resumes rather than
//! restarting. Work is deferred, never dropped.
//!
//! One bound is missing, and it is not the sweeper's to fix. `BackgroundTransfer::run`, which both
//! archive triggers share, reads an agent's whole layer into one `Vec` before appending it. So the
//! sweeper's peak memory is `max_concurrency` agents' layers, not `max_concurrency` times a chunk
//! size. In practice the size trigger at `entry_count_limit` keeps a quiet agent's residue small,
//! but the ceiling is the layer, not a constant. Chunking `run` fixes it for the size-triggered
//! path at the same time, and belongs in its own change with its own tests.

use std::collections::{BTreeMap, HashMap};
use std::fmt::{self, Display};
use std::sync::Arc;
use std::time::Instant;

use futures::stream::{self, StreamExt};
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{AgentId, OwnedAgentId, ShardAssignment, ShardId};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, info, info_span, warn};
use uuid::Uuid;

use crate::metrics::oplog::{record_oplog_sweep_outcome, record_oplog_sweep_tick};
use crate::services::golem_config::OplogSweepConfig;
use crate::services::oplog::{EphemeralOplog, MultiLayerOplog, OplogArchiveService};
use crate::services::scheduler::SchedulerWorkerAccess;
use crate::services::shard::ShardService;
use crate::storage::indexed::{
    IndexedStorage, IndexedStorageLabelledApi, IndexedStorageMetaNamespace, ScanCursor,
};

/// One archive step the sweeper performs: entries for agents of `agent_mode` move out of the layer
/// at `source_level` into the layer below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RouteId {
    pub agent_mode: AgentMode,
    pub source_level: usize,
}

impl Display for RouteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mode = match self.agent_mode {
            AgentMode::Durable => "durable",
            AgentMode::Ephemeral => "ephemeral",
        };
        write!(f, "{mode}-l{}", self.source_level)
    }
}

/// What the sweeper decided about one scanned key. Every scanned key produces exactly one of
/// these, which is what makes [`tally`] a total fold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The key did not parse as `{component_id}:{agent_name}`.
    Unparseable,
    /// Another executor owns this agent's shard.
    NotOwned,
    /// The layer holds nothing. The key lost its entries between the scan and the probe.
    Empty,
    /// The layer's last index moved since the previous tick, so something is still writing.
    Moving,
    /// This executor is running the agent right now.
    Resident,
    /// The layer holds no `Create` entry, so the target address cannot be recovered.
    Unaddressable,
    /// Opening the oplog failed.
    OpenFailed,
    /// One archive step ran. `more` carries the `try_archive` contract: a layer below this one
    /// still holds entries for this agent, so a later tick has work to do.
    Archived { more: bool },
}

/// Whether an agent has been quiet long enough to archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Remember this index and reconsider on the next tick.
    Wait,
    /// The index has not moved since the previous tick. Archive.
    Move,
}

/// Per-route counters. Every field counts scanned keys except `entries` and `drained`, and the
/// counted fields sum to `scanned`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteReport {
    pub scanned: u64,
    pub unparseable: u64,
    pub not_owned: u64,
    pub empty: u64,
    pub moving: u64,
    pub resident: u64,
    pub unaddressable: u64,
    pub open_failed: u64,
    pub archived: u64,
    /// Archive steps that reported no further layer to drain. A subset of `archived`.
    pub drained: u64,
    /// True when a budget stopped the tick before the scan reached the end of the namespace.
    pub truncated: bool,
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
            ("open_failed", self.open_failed),
            ("archived", self.archived),
            ("drained", self.drained),
        ] {
            record_oplog_sweep_outcome(route, outcome, count);
        }
        record_oplog_sweep_tick(route, elapsed, self.truncated);
    }
}

/// What one call to [`OplogSweeper::sweep_once`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub routes: Vec<(RouteId, RouteReport)>,
    /// True when the sweeper ran without a shard assignment, in which case it did nothing.
    pub unassigned: bool,
}

impl SweepReport {
    pub fn route(&self, id: RouteId) -> RouteReport {
        self.routes
            .iter()
            .find(|(route, _)| *route == id)
            .map(|(_, report)| report.clone())
            .unwrap_or_default()
    }

    pub fn archived(&self) -> u64 {
        self.routes.iter().map(|(_, r)| r.archived).sum()
    }

    pub fn scanned(&self) -> u64 {
        self.routes.iter().map(|(_, r)| r.scanned).sum()
    }
}

/// Layer keys are `{component_id}:{agent_name}`, written by `AgentId::to_redis_key`. An agent name
/// may itself contain `:`, so only the first separator is significant.
pub fn parse_agent_id(key: &str) -> Option<AgentId> {
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
pub fn owns(assignment: &ShardAssignment, agent_id: &AgentId) -> bool {
    let shard_id = ShardId::from_routing_hash(
        ShardId::hash_agent_id(agent_id),
        assignment.number_of_shards,
    );
    assignment.shard_ids.contains(&shard_id)
}

/// The quiet gate.
///
/// `ScheduledAction::ArchiveOplog` carries the index the agent had when the action was registered
/// and acts only if the current index still matches. The sweeper has no registration to carry an
/// index, so it compares against what it saw on its own previous tick. An agent therefore becomes
/// eligible after its index survives one whole interval unchanged.
pub fn assess(remembered: Option<OplogIndex>, current: OplogIndex) -> Verdict {
    match remembered {
        Some(previous) if previous == current => Verdict::Move,
        _ => Verdict::Wait,
    }
}

/// The environment an agent's entries belong to.
///
/// Reading an indexed layer needs only an `AgentId`, but opening the oplog needs an
/// `OwnedAgentId`, and the blob layer below addresses its objects by environment. The `Create`
/// entry carries it, so the entries being moved are their own address book.
pub fn environment_of(entries: &BTreeMap<OplogIndex, OplogEntry>) -> Option<EnvironmentId> {
    entries.values().find_map(|entry| match entry {
        OplogEntry::Create { environment_id, .. } => Some(*environment_id),
        _ => None,
    })
}

/// Adds two reports. `truncated` is sticky: a tick that hit a budget on any page hit it.
pub fn merge(left: RouteReport, right: RouteReport) -> RouteReport {
    RouteReport {
        scanned: left.scanned + right.scanned,
        unparseable: left.unparseable + right.unparseable,
        not_owned: left.not_owned + right.not_owned,
        empty: left.empty + right.empty,
        moving: left.moving + right.moving,
        resident: left.resident + right.resident,
        unaddressable: left.unaddressable + right.unaddressable,
        open_failed: left.open_failed + right.open_failed,
        archived: left.archived + right.archived,
        drained: left.drained + right.drained,
        truncated: left.truncated || right.truncated,
    }
}

/// Folds per-key outcomes into a report.
pub fn tally(outcomes: impl IntoIterator<Item = Outcome>) -> RouteReport {
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
                Outcome::OpenFailed => report.open_failed += 1,
                Outcome::Archived { more } => {
                    report.archived += 1;
                    if !more {
                        report.drained += 1;
                    }
                }
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

struct Route {
    id: RouteId,
    namespace: IndexedStorageMetaNamespace,
    source: Arc<dyn OplogArchiveService>,
}

/// A running tick loop.
///
/// The loop stops when the token given to [`OplogSweeper::spawn`] is cancelled, or when
/// [`stop`](Self::stop) is called. Dropping this handle leaves the loop running: the executor's
/// other background loops, `SchedulerServiceDefault` and `AgentStatusFlushQueue`, are governed by
/// their shutdown token the same way, and threading a drop guard through `All` would put a
/// lifetime holder into a thirty-argument constructor for no gain.
pub struct SweepLoop {
    stop: CancellationToken,
}

impl SweepLoop {
    /// Stops the loop after the tick it is currently running.
    pub fn stop(&self) {
        self.stop.cancel();
    }
}

pub struct OplogSweeper {
    config: OplogSweepConfig,
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    routes: Vec<Route>,
    shards: Arc<dyn ShardService>,
    worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync>,
    /// The index each agent showed on the previous tick, per route. Losing it costs one extra
    /// tick of latency, never a stranded oplog: the work list comes from storage.
    memo: Mutex<HashMap<(RouteId, AgentId), OplogIndex>>,
    /// Where each route's scan stopped, so a budgeted tick resumes rather than restarting.
    cursors: Mutex<HashMap<RouteId, ScanCursor>>,
}

impl OplogSweeper {
    /// Derives its routes from the layer stack `lib.rs` already built.
    ///
    /// A layer is a source when it can enumerate its own keys and something sits below it to
    /// receive them. Blob-backed archives answer `None` to `scan_namespace`, so the bottom of the
    /// stack is a target only, and no route can source from it.
    ///
    /// Pure: no I/O, no task, no runtime needed. Call [`spawn`](Self::spawn) to start ticking.
    pub fn over_layers(
        config: OplogSweepConfig,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        archives: &[Arc<dyn OplogArchiveService>],
        shards: Arc<dyn ShardService>,
        worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync>,
    ) -> Arc<Self> {
        // The bottom layer receives entries and has nowhere to pass them on to, so it is never a
        // source.
        let sources = archives.len().saturating_sub(1);
        let modes = config.agent_modes();
        let mut routes: Vec<Route> = Vec::new();
        for source in archives.iter().take(sources) {
            for agent_mode in &modes {
                let Some(namespace) = source.scan_namespace(*agent_mode) else {
                    continue;
                };
                let source_level = match namespace {
                    IndexedStorageMetaNamespace::CompressedOplog { level, .. } => level,
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
            worker_access,
            memo: Mutex::new(HashMap::new()),
            cursors: Mutex::new(HashMap::new()),
        })
    }

    /// Starts the tick loop, which runs until the token is cancelled. A no-op when the config is
    /// disabled or no route exists, in which case `ScheduledAction::ArchiveOplog` remains the only
    /// archiving mechanism.
    pub fn spawn(self: &Arc<Self>, shutdown: CancellationToken) -> SweepLoop {
        let stop = shutdown.child_token();
        if !self.config.enabled || self.routes.is_empty() {
            return SweepLoop { stop };
        }

        let sweeper = self.clone();
        let ticking = stop.clone();
        let interval = self.config.interval;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = ticking.cancelled() => break,
                    _ = tokio::time::sleep(interval) => {}
                }
                if ticking.is_cancelled() {
                    break;
                }
                let report = sweeper.sweep_once().await;
                if report.archived() > 0 {
                    debug!(
                        archived = report.archived(),
                        scanned = report.scanned(),
                        "Oplog sweep tick"
                    );
                }
            }
        });

        SweepLoop { stop }
    }

    /// Runs one pass over every route. Never fails: a storage error ends the affected route's tick
    /// and the next one retries, because the work list is the layer itself.
    pub async fn sweep_once(&self) -> SweepReport {
        let Some(assignment) = self.shards.try_get_current_assignment() else {
            // Without an assignment every agent would look like someone else's, and archiving an
            // agent this executor may not own is exactly what the shard check is for.
            return SweepReport {
                routes: Vec::new(),
                unassigned: true,
            };
        };

        let mut routes = Vec::with_capacity(self.routes.len());
        for route in &self.routes {
            let report = self
                .sweep_route(route, &assignment)
                .instrument(info_span!("oplog_sweep", route = %route.id))
                .await;
            routes.push((route.id, report));
        }
        SweepReport {
            routes,
            unassigned: false,
        }
    }

    async fn sweep_route(&self, route: &Route, assignment: &ShardAssignment) -> RouteReport {
        let started = Instant::now();
        let mut cursor = self
            .cursors
            .lock()
            .await
            .get(&route.id)
            .copied()
            .unwrap_or(0);
        // Folded page by page: holding every outcome would make a tick's memory grow with the
        // namespace, which is the thing the budgets exist to prevent.
        let mut report = RouteReport::default();
        let mut truncated = false;

        loop {
            let page = self
                .indexed_storage
                .with("oplog_sweep", "scan")
                .scan(route.namespace.clone(), None, cursor, self.config.page_size)
                .await;

            let (next_cursor, keys) = match page {
                Ok(page) => page,
                Err(error) => {
                    warn!(route = %route.id, "Oplog sweep scan failed: {error}");
                    break;
                }
            };

            let (candidates, settled) = triage(&keys, assignment);
            let probed: Vec<Outcome> = stream::iter(candidates)
                .map(|agent_id| self.sweep_agent(route, agent_id))
                .buffer_unordered(self.config.max_concurrency.max(1))
                .collect()
                .await;
            report = merge(report, tally(settled.into_iter().chain(probed)));

            cursor = next_cursor;
            if cursor == 0 {
                break;
            }
            if report.archived >= self.config.max_archives_per_tick as u64
                || report.scanned >= self.config.max_scanned_per_tick as u64
            {
                truncated = true;
                break;
            }
        }

        self.cursors.lock().await.insert(route.id, cursor);
        self.forget_stale(route.id).await;

        report.truncated = truncated;
        report.record(&route.id.to_string(), started.elapsed());
        report
    }

    /// Decides one agent and, if it is quiet, runs a single archive step against it.
    async fn sweep_agent(&self, route: &Route, agent_id: AgentId) -> Outcome {
        // Reading an indexed layer is keyed by agent and mode only, so any environment addresses
        // the same rows. The real one is recovered from the `Create` entry below, before anything
        // that needs it.
        let probe = OwnedAgentId {
            environment_id: EnvironmentId::new(),
            agent_id: agent_id.clone(),
        };

        let current = route
            .source
            .get_last_index(&probe, route.id.agent_mode)
            .await;
        if current == OplogIndex::NONE {
            self.forget(route.id, &agent_id).await;
            return Outcome::Empty;
        }

        let remembered = self
            .memo
            .lock()
            .await
            .get(&(route.id, agent_id.clone()))
            .copied();
        if assess(remembered, current) == Verdict::Wait {
            self.remember(route.id, &agent_id, current).await;
            return Outcome::Moving;
        }

        let head = route
            .source
            .read(&probe, route.id.agent_mode, OplogIndex::INITIAL, 1)
            .await;
        let Some(environment_id) = environment_of(&head) else {
            return Outcome::Unaddressable;
        };
        let owned_agent_id = OwnedAgentId {
            environment_id,
            agent_id: agent_id.clone(),
        };

        // Cheap pre-filter for the expensive check below. An agent this executor is running has a
        // live `Worker` holding the oplog, and opening it would only queue behind that.
        if self
            .worker_access
            .active_worker_fingerprint(&owned_agent_id)
            .await
            .is_some()
        {
            return Outcome::Resident;
        }

        // Building the suspended `Worker` is the mutual exclusion, exactly as it is for
        // `ScheduledAction::ArchiveOplog`. It costs one construction per agent that genuinely has
        // a tail to move, where the scheduled action paid one per registered row.
        let oplog = match self.worker_access.open_oplog(&owned_agent_id).await {
            Ok(oplog) => oplog,
            Err(error) => {
                warn!(
                    agent_id = %agent_id,
                    "Oplog sweep could not open the oplog for archiving: {error}"
                );
                return Outcome::OpenFailed;
            }
        };

        let more = match MultiLayerOplog::try_archive_blocking(&oplog).await {
            Some(more) => more,
            None => EphemeralOplog::try_archive_blocking(&oplog)
                .await
                .unwrap_or(false),
        };

        self.forget(route.id, &agent_id).await;
        debug!(agent_id = %agent_id, more, "Oplog sweep archived one layer");
        Outcome::Archived { more }
    }

    async fn remember(&self, route: RouteId, agent_id: &AgentId, index: OplogIndex) {
        let mut memo = self.memo.lock().await;
        if memo.len() >= self.config.max_tracked_agents {
            // The memo only defers work, so shedding it costs one extra tick for whichever agents
            // lose their entry. Clearing beats evicting arbitrarily: it keeps the bound obvious.
            warn!(
                tracked = memo.len(),
                "Oplog sweep tracking table full, clearing it"
            );
            memo.clear();
        }
        memo.insert((route, agent_id.clone()), index);
    }

    async fn forget(&self, route: RouteId, agent_id: &AgentId) {
        self.memo.lock().await.remove(&(route, agent_id.clone()));
    }

    /// Drops memo entries for agents this executor no longer owns, so a reshard does not leave
    /// them behind forever.
    async fn forget_stale(&self, route: RouteId) {
        let Some(assignment) = self.shards.try_get_current_assignment() else {
            return;
        };
        self.memo.lock().await.retain(|(memo_route, agent_id), _| {
            *memo_route != route || owns(&assignment, agent_id)
        });
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
    use async_trait::async_trait;
    use golem_common::model::account::AccountId;
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::oplog::OplogEntry;
    use golem_common::model::{
        AgentFingerprint, AgentInvocation, AgentMetadata, AgentStatusRecord, RetryConfig, Timestamp,
    };
    use golem_common::read_only_lock;
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use nonempty_collections::nev;
    use std::collections::HashSet;
    use std::sync::RwLock;
    use test_r::test;
    use uuid::Uuid;

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
    fn an_agent_is_quiet_only_after_its_index_survives_a_tick() {
        let first = OplogIndex::from_u64(11);
        let second = OplogIndex::from_u64(12);

        assert_eq!(assess(None, first), Verdict::Wait);
        assert_eq!(assess(Some(first), second), Verdict::Wait);
        assert_eq!(assess(Some(second), second), Verdict::Move);
    }

    #[test]
    fn environment_comes_from_the_create_entry() {
        let environment_id = EnvironmentId::new();
        let agent_id = agent("counter-1", ComponentId::new());

        let mut with_create = BTreeMap::new();
        with_create.insert(OplogIndex::INITIAL, create_entry(&agent_id, environment_id));
        assert_eq!(environment_of(&with_create), Some(environment_id));

        let mut without_create = BTreeMap::new();
        without_create.insert(OplogIndex::INITIAL, OplogEntry::suspend());
        assert_eq!(environment_of(&without_create), None);
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
            Outcome::OpenFailed,
            Outcome::Archived { more: true },
            Outcome::Archived { more: false },
        ]);

        assert_eq!(report.scanned, 9);
        assert_eq!(report.archived, 2);
        assert_eq!(report.drained, 1);
        assert_eq!(
            report.unparseable
                + report.not_owned
                + report.empty
                + report.moving
                + report.resident
                + report.unaddressable
                + report.open_failed
                + report.archived,
            report.scanned
        );
    }

    #[test]
    fn merge_adds_reports_and_keeps_truncation_sticky() {
        let left = tally([Outcome::Moving, Outcome::Archived { more: false }]);
        let mut right = tally([Outcome::Resident]);
        right.truncated = true;

        let merged = merge(left, right);
        assert_eq!(merged.scanned, 3);
        assert_eq!(merged.moving, 1);
        assert_eq!(merged.archived, 1);
        assert_eq!(merged.drained, 1);
        assert_eq!(merged.resident, 1);
        assert!(merged.truncated);
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

    /// Opens the oplog directly instead of building a `Worker` around it. The production adapter
    /// is `Arc<dyn WorkerActivator<Ctx>>`, whose `open_oplog` goes through
    /// `get_or_create_suspended`; both hand the sweeper the same `Arc<dyn Oplog>`.
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

        async fn activate_worker(&self, _owned_agent_id: &OwnedAgentId) {}

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
                    read_only_lock::tokio::ReadOnlyLock::new(Arc::new(tokio::sync::RwLock::new(
                        AgentStatusRecord::default(),
                    ))),
                    read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(
                        ExecutionStatus::Suspended {
                            agent_mode: AgentMode::Ephemeral,
                            timestamp: Timestamp::now_utc(),
                        },
                    ))),
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

    fn shards_owning(agent_id: &AgentId) -> Arc<dyn ShardService> {
        let shard_service = Arc::new(ShardServiceDefault::new());
        shard_service.register(
            1,
            &HashSet::from([ShardId::from_routing_hash(
                ShardId::hash_agent_id(agent_id),
                1,
            )]),
        );
        shard_service
    }

    fn sweeper(
        layers: &Layers,
        agent_id: &AgentId,
        resident: HashSet<AgentId>,
    ) -> Arc<OplogSweeper> {
        OplogSweeper::over_layers(
            OplogSweepConfig {
                enabled: false, // no tick loop; the test drives `sweep_once` itself
                ..OplogSweepConfig::default()
            },
            layers.indexed_storage.clone(),
            &layers.archives,
            shards_owning(agent_id),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                resident,
            }),
        )
    }

    async fn stranded_ephemeral_oplog(layers: &Layers, agent_id: &AgentId) -> EnvironmentId {
        let environment_id = EnvironmentId::new();
        let owned_agent_id = OwnedAgentId::new(environment_id, agent_id);
        let oplog = layers
            .oplog_service
            .create(
                &owned_agent_id,
                AgentMode::Ephemeral,
                create_entry(agent_id, environment_id),
                metadata(agent_id, environment_id),
                read_only_lock::tokio::ReadOnlyLock::new(Arc::new(tokio::sync::RwLock::new(
                    AgentStatusRecord::default(),
                ))),
                read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(
                    ExecutionStatus::Suspended {
                        agent_mode: AgentMode::Ephemeral,
                        timestamp: Timestamp::now_utc(),
                    },
                ))),
            )
            .await;
        oplog.add(OplogEntry::suspend()).await;
        oplog.add(OplogEntry::exited()).await;
        oplog.commit(CommitLevel::Always).await;
        // The agent is gone. Nothing drained its layer, which is the state a crash between the
        // last commit and `archive_ephemeral_oplog` leaves behind.
        drop(oplog);
        environment_id
    }

    #[test]
    async fn the_bottom_layer_is_never_a_source() {
        let layers = layers();
        let agent_id = agent("counter-1", ComponentId::new());
        let sweeper = sweeper(&layers, &agent_id, HashSet::new());

        // Two archives, one route: the blob layer receives entries and cannot enumerate its own.
        assert_eq!(sweeper.routes.len(), 1);
        assert_eq!(
            sweeper.routes[0].id,
            RouteId {
                agent_mode: AgentMode::Ephemeral,
                source_level: 1
            }
        );
    }

    #[test]
    async fn a_quiet_ephemeral_layer_is_archived_on_the_second_tick() {
        let layers = layers();
        let agent_id = agent("counter-1", ComponentId::new());
        let environment_id = stranded_ephemeral_oplog(&layers, &agent_id).await;
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        let route = RouteId {
            agent_mode: AgentMode::Ephemeral,
            source_level: 1,
        };

        let sweeper = sweeper(&layers, &agent_id, HashSet::new());

        // An ephemeral oplog never reaches the primary: its entries start in the first lower
        // layer, and the layer below is empty.
        let source_before = layers.archives[0]
            .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
            .await;
        let target_before = layers.archives[1]
            .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
            .await;
        assert_eq!(source_before, OplogIndex::from_u64(3));
        assert_eq!(target_before, OplogIndex::NONE);

        // The first tick has nothing to compare against, so it only records the index.
        let first = sweeper.sweep_once().await;
        assert_eq!(first.route(route).scanned, 1);
        assert_eq!(first.route(route).moving, 1);
        assert_eq!(first.route(route).archived, 0);

        // The index has not moved, so the second tick archives.
        let second = sweeper.sweep_once().await;
        assert_eq!(second.route(route).archived, 1);

        // `drop_prefix` removes the key, so the layer no longer enumerates the agent at all. The
        // scan returns work, not agents.
        let third = sweeper.sweep_once().await;
        assert_eq!(third.route(route).scanned, 0);

        // The entries changed layer, which is what "archived" has to mean.
        let source_after = layers.archives[0]
            .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
            .await;
        let target_after = layers.archives[1]
            .get_last_index(&owned_agent_id, AgentMode::Ephemeral)
            .await;
        assert_eq!(source_after, OplogIndex::NONE);
        assert_eq!(target_after, OplogIndex::from_u64(3));

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

    #[test]
    async fn an_agent_this_executor_is_running_is_left_alone() {
        let layers = layers();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id).await;
        let route = RouteId {
            agent_mode: AgentMode::Ephemeral,
            source_level: 1,
        };

        let sweeper = sweeper(&layers, &agent_id, HashSet::from([agent_id.clone()]));

        sweeper.sweep_once().await;
        let second = sweeper.sweep_once().await;

        assert_eq!(second.route(route).resident, 1);
        assert_eq!(second.route(route).archived, 0);
    }

    #[test]
    async fn an_agent_on_another_shard_is_left_alone() {
        let layers = layers();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id).await;
        let route = RouteId {
            agent_mode: AgentMode::Ephemeral,
            source_level: 1,
        };

        let shard_service = Arc::new(ShardServiceDefault::new());
        shard_service.register(4, &HashSet::new());
        let sweeper = OplogSweeper::over_layers(
            OplogSweepConfig {
                enabled: false,
                ..OplogSweepConfig::default()
            },
            layers.indexed_storage.clone(),
            &layers.archives,
            shard_service,
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                resident: HashSet::new(),
            }),
        );

        sweeper.sweep_once().await;
        let second = sweeper.sweep_once().await;

        assert_eq!(second.route(route).not_owned, 1);
        assert_eq!(second.route(route).archived, 0);
    }

    #[test]
    async fn a_budgeted_tick_resumes_where_it_stopped() {
        let layers = layers();
        let component_id = ComponentId::new();
        let agents: Vec<AgentId> = (0..3)
            .map(|i| agent(&format!("counter-{i}"), component_id))
            .collect();
        for agent_id in &agents {
            stranded_ephemeral_oplog(&layers, agent_id).await;
        }
        let route = RouteId {
            agent_mode: AgentMode::Ephemeral,
            source_level: 1,
        };

        // One key per scan call, one key per tick: three ticks to see three agents.
        let sweeper = OplogSweeper::over_layers(
            OplogSweepConfig {
                enabled: false,
                page_size: 1,
                max_scanned_per_tick: 1,
                ..OplogSweepConfig::default()
            },
            layers.indexed_storage.clone(),
            &layers.archives,
            shards_owning(&agents[0]),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                resident: HashSet::new(),
            }),
        );

        let mut seen = 0;
        for _ in 0..3 {
            let report = sweeper.sweep_once().await;
            assert!(report.route(route).truncated);
            seen += report.route(route).scanned;
        }

        // Each tick stopped after its budget and the next one carried on, so three ticks covered
        // all three agents rather than revisiting the first.
        assert_eq!(seen, 3);
        assert_eq!(sweeper.memo.lock().await.len(), 3);
    }

    #[test]
    async fn a_sweep_without_a_shard_assignment_does_nothing() {
        let layers = layers();
        let agent_id = agent("counter-1", ComponentId::new());
        stranded_ephemeral_oplog(&layers, &agent_id).await;

        let sweeper = OplogSweeper::over_layers(
            OplogSweepConfig {
                enabled: false,
                ..OplogSweepConfig::default()
            },
            layers.indexed_storage.clone(),
            &layers.archives,
            Arc::new(ShardServiceDefault::new()),
            Arc::new(DirectAccess {
                oplog_service: layers.oplog_service.clone(),
                resident: HashSet::new(),
            }),
        );

        let report = sweeper.sweep_once().await;
        assert!(report.unassigned);
        assert_eq!(report.scanned(), 0);
    }
}
