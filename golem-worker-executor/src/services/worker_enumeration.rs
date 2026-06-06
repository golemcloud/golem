use crate::services::active_workers::ActiveWorkers;
use crate::services::component::ComponentService;
use crate::services::golem_config::GolemConfig;
use crate::services::oplog::OplogService;
use crate::services::worker::WorkerService;
use crate::services::{HasComponentService, HasConfig, HasOplogService, HasWorkerService};
use crate::worker::status::calculate_last_known_status_with_checkpoint;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::base_model::worker_filter::{
    AgentAndFilter, AgentModeFilter, AgentNotFilter, AgentOrFilter, FilterComparator,
};
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentFilter, AgentMetadata, AgentStatus, ScanCursor};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::sync::Arc;
use tracing::{Instrument, info};

/// Returns `true` if `filter` contains any `Mode(...)` reference at any depth.
fn contains_mode(filter: &AgentFilter) -> bool {
    match filter {
        AgentFilter::Mode(_) => true,
        AgentFilter::And(AgentAndFilter { filters })
        | AgentFilter::Or(AgentOrFilter { filters }) => filters.iter().any(contains_mode),
        AgentFilter::Not(AgentNotFilter { filter }) => contains_mode(filter),
        AgentFilter::Name(_)
        | AgentFilter::Status(_)
        | AgentFilter::Revision(_)
        | AgentFilter::CreatedAt(_)
        | AgentFilter::Env(_)
        | AgentFilter::Config(_) => false,
    }
}

/// Derives the `modes` argument for `OplogService::scan_for_component` from the
/// optional user-supplied `AgentFilter`.
///
/// The default behaviour is to scan only durable agents so that ephemeral
/// agents from previous runs do not appear in the default agent listing.
/// This default applies whenever the user-supplied filter does not constrain
/// the agent mode in any way (including when no filter is supplied at all, or
/// when the filter only constrains other properties such as name, status,
/// env, etc.).
///
/// Callers can override the default by including an explicit `mode == ...`
/// constraint.
///
/// Rules:
/// - `None` → `Some(AgentMode::Durable)` (default)
/// - Filter contains no `Mode(...)` reference at any depth → `Some(Durable)`
///   (default applies even when other constraints are present)
/// - Top-level `Mode(Equal, m)` → `Some(m)`
/// - `And(filters)` containing exactly one top-level `Mode(Equal, m)`
///   constraint, where no other top-level child contains any nested `Mode`
///   reference → `Some(m)` (other filters in the AND do not affect the
///   storage-level mode selection but are still applied post-scan)
/// - Anything else (Or, Not, NotEqual on Mode, multiple distinct Mode
///   constraints, nested Mode constraints, ...) → `None` (scan both modes,
///   the existing post-scan `filter.matches(&metadata)` trims results)
pub(crate) fn modes_from_filter(filter: &Option<AgentFilter>) -> Option<AgentMode> {
    let Some(filter) = filter else {
        return Some(AgentMode::Durable);
    };

    // If the filter doesn't reference Mode at all, the default applies even
    // when other constraints (name, status, ...) are present.
    if !contains_mode(filter) {
        return Some(AgentMode::Durable);
    }

    match filter {
        AgentFilter::Mode(AgentModeFilter {
            comparator: FilterComparator::Equal,
            value,
        }) => Some(*value),
        AgentFilter::And(AgentAndFilter { filters }) => {
            let mut mode_eq: Option<AgentMode> = None;
            for f in filters {
                match f {
                    AgentFilter::Mode(AgentModeFilter {
                        comparator: FilterComparator::Equal,
                        value,
                    }) => match mode_eq {
                        Some(prev) if prev != *value => {
                            // Distinct top-level Mode(Equal, ..) constraints
                            // contradict each other; let the post-scan matcher
                            // decide (it will simply return no matches).
                            return None;
                        }
                        _ => mode_eq = Some(*value),
                    },
                    // Any other top-level Mode comparator (e.g. NotEqual) cannot
                    // be safely narrowed.
                    AgentFilter::Mode(_) => return None,
                    // Non-Mode child: must not hide a Mode reference inside,
                    // otherwise narrowing the scan could drop valid matches.
                    other => {
                        if contains_mode(other) {
                            return None;
                        }
                    }
                }
            }
            // `contains_mode(filter)` was true above and every nested-Mode case
            // has already short-circuited, so a top-level Mode(Equal, ..) must
            // have been captured.
            mode_eq.map(Some).unwrap_or(None)
        }
        _ => None,
    }
}

#[async_trait]
pub trait RunningWorkerEnumerationService: Send + Sync {
    async fn get(
        &self,
        component_id: &ComponentId,
        filter: Option<AgentFilter>,
    ) -> Result<Vec<AgentMetadata>, WorkerExecutorError>;
}

#[derive(Clone)]
pub struct RunningWorkerEnumerationServiceDefault<Ctx: WorkerCtx> {
    active_workers: Arc<ActiveWorkers<Ctx>>,
}

#[async_trait]
impl<Ctx: WorkerCtx> RunningWorkerEnumerationService
    for RunningWorkerEnumerationServiceDefault<Ctx>
{
    async fn get(
        &self,
        component_id: &ComponentId,
        filter: Option<AgentFilter>,
    ) -> Result<Vec<AgentMetadata>, WorkerExecutorError> {
        info!(
            "Get workers - filter: {}",
            filter
                .clone()
                .map(|f| f.to_string())
                .unwrap_or("N/A".to_string())
        );

        let active_workers = self.active_workers.snapshot().await;

        let mut workers: Vec<AgentMetadata> = vec![];
        for (agent_id, worker) in active_workers {
            let metadata = worker.get_latest_worker_metadata().await;
            if agent_id.component_id == *component_id
                && (metadata.last_known_status.status == AgentStatus::Running)
                && filter.clone().is_none_or(|f| f.matches(&metadata))
            {
                workers.push(metadata);
            }
        }

        Ok(workers)
    }
}

impl<Ctx: WorkerCtx> RunningWorkerEnumerationServiceDefault<Ctx> {
    pub fn new(active_workers: Arc<ActiveWorkers<Ctx>>) -> Self {
        Self { active_workers }
    }
}

#[async_trait]
pub trait WorkerEnumerationService: Send + Sync {
    async fn get(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        filter: Option<AgentFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> Result<(Option<ScanCursor>, Vec<AgentMetadata>), WorkerExecutorError>;
}

#[derive(Clone)]
pub struct DefaultWorkerEnumerationService {
    worker_service: Arc<dyn WorkerService>,
    oplog_service: Arc<dyn OplogService>,
    component_service: Arc<dyn ComponentService>,
    golem_config: Arc<GolemConfig>,
}

impl DefaultWorkerEnumerationService {
    pub fn new(
        worker_service: Arc<dyn WorkerService>,
        oplog_service: Arc<dyn OplogService>,
        component_service: Arc<dyn ComponentService>,
        golem_config: Arc<GolemConfig>,
    ) -> Self {
        Self {
            worker_service,
            oplog_service,
            component_service,
            golem_config,
        }
    }

    async fn get_internal(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        filter: Option<AgentFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> Result<(Option<ScanCursor>, Vec<AgentMetadata>), WorkerExecutorError> {
        let mut workers: Vec<AgentMetadata> = vec![];

        let modes = modes_from_filter(&filter);
        let (new_cursor, keys) = self
            .oplog_service
            .scan_for_component(environment_id, component_id, modes, cursor, count)
            .instrument(tracing::info_span!("scan_for_component"))
            .await?;

        for owned_agent_id in keys {
            let worker_metadata = self
                .worker_service
                .get(&owned_agent_id)
                .instrument(tracing::info_span!("get_worker_metadata"))
                .await;

            if let Some(worker_metadata) = worker_metadata {
                let metadata = if precise {
                    let agent_mode = worker_metadata.initial_worker_metadata.agent_mode;
                    let last_known_status = calculate_last_known_status_with_checkpoint(
                        self,
                        &owned_agent_id,
                        agent_mode,
                        worker_metadata.last_known_status,
                    )
                    .instrument(tracing::info_span!("calculate_last_known_status"))
                    .await
                    .expect("Failed to calculate worker status for existing worker");

                    AgentMetadata {
                        last_known_status,
                        ..worker_metadata.initial_worker_metadata
                    }
                } else {
                    AgentMetadata {
                        last_known_status: worker_metadata.last_known_status.unwrap_or_default(),
                        ..worker_metadata.initial_worker_metadata
                    }
                };

                if filter.clone().is_none_or(|f| f.matches(&metadata)) {
                    workers.push(metadata);
                }
            }
        }

        Ok((new_cursor.into_option(), workers))
    }
}

impl HasOplogService for DefaultWorkerEnumerationService {
    fn oplog_service(&self) -> Arc<dyn OplogService> {
        self.oplog_service.clone()
    }
}

impl HasWorkerService for DefaultWorkerEnumerationService {
    fn worker_service(&self) -> Arc<dyn WorkerService> {
        self.worker_service.clone()
    }
}

impl HasConfig for DefaultWorkerEnumerationService {
    fn config(&self) -> Arc<GolemConfig> {
        self.golem_config.clone()
    }
}

impl HasComponentService for DefaultWorkerEnumerationService {
    fn component_service(&self) -> Arc<dyn ComponentService> {
        self.component_service.clone()
    }
}

#[async_trait]
impl WorkerEnumerationService for DefaultWorkerEnumerationService {
    async fn get(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        filter: Option<AgentFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> Result<(Option<ScanCursor>, Vec<AgentMetadata>), WorkerExecutorError> {
        info!(
            environment_id = %environment_id,
            component_id = %component_id,
            filter = filter
                .clone()
                .map(|f| f.to_string())
                .unwrap_or("N/A".to_string()),
            cursor = %cursor,
            count = %count,
            precise = %precise,
            "Enumerating workers"
        );
        let mut new_cursor: Option<ScanCursor> = Some(cursor);
        let mut workers: Vec<AgentMetadata> = vec![];

        while new_cursor.is_some() && (workers.len() as u64) < count {
            let new_count = count - (workers.len() as u64);

            let (next_cursor, workers_page) = self
                .get_internal(
                    environment_id,
                    component_id,
                    filter.clone(),
                    new_cursor.unwrap_or_default(),
                    new_count,
                    precise,
                )
                .await?;

            workers.extend(workers_page);

            new_cursor = next_cursor;
        }

        Ok((new_cursor, workers))
    }
}

#[cfg(test)]
mod tests {
    use super::modes_from_filter;
    use golem_common::base_model::worker_filter::{FilterComparator, StringFilterComparator};
    use golem_common::model::AgentFilter;
    use golem_common::model::agent::AgentMode;
    use test_r::test;

    #[test]
    fn no_filter_defaults_to_durable() {
        assert_eq!(modes_from_filter(&None), Some(AgentMode::Durable));
    }

    #[test]
    fn top_level_mode_equal_durable() {
        let f = AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Durable);
        assert_eq!(modes_from_filter(&Some(f)), Some(AgentMode::Durable));
    }

    #[test]
    fn top_level_mode_equal_ephemeral() {
        let f = AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Ephemeral);
        assert_eq!(modes_from_filter(&Some(f)), Some(AgentMode::Ephemeral));
    }

    #[test]
    fn top_level_mode_not_equal_returns_none() {
        let f = AgentFilter::new_mode(FilterComparator::NotEqual, AgentMode::Durable);
        assert_eq!(modes_from_filter(&Some(f)), None);
    }

    #[test]
    fn and_with_no_mode_constraint_defaults_to_durable() {
        let f = AgentFilter::new_and(vec![AgentFilter::new_name(
            StringFilterComparator::Equal,
            "x".to_string(),
        )]);
        assert_eq!(modes_from_filter(&Some(f)), Some(AgentMode::Durable));
    }

    #[test]
    fn and_with_single_mode_equal_uses_it() {
        let f = AgentFilter::new_and(vec![
            AgentFilter::new_name(StringFilterComparator::Equal, "x".to_string()),
            AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Ephemeral),
        ]);
        assert_eq!(modes_from_filter(&Some(f)), Some(AgentMode::Ephemeral));
    }

    #[test]
    fn and_with_duplicate_identical_mode_equal_uses_it() {
        // Two identical top-level Mode(Equal, ..) constraints are logically
        // equivalent to a single one and can be narrowed safely.
        let f = AgentFilter::new_and(vec![
            AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Durable),
            AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Durable),
        ]);
        assert_eq!(modes_from_filter(&Some(f)), Some(AgentMode::Durable));
    }

    #[test]
    fn and_with_two_distinct_mode_equal_returns_none() {
        let f = AgentFilter::new_and(vec![
            AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Durable),
            AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Ephemeral),
        ]);
        assert_eq!(modes_from_filter(&Some(f)), None);
    }

    #[test]
    fn and_with_mode_not_equal_returns_none() {
        let f = AgentFilter::new_and(vec![AgentFilter::new_mode(
            FilterComparator::NotEqual,
            AgentMode::Durable,
        )]);
        assert_eq!(modes_from_filter(&Some(f)), None);
    }

    #[test]
    fn or_returns_none() {
        let f = AgentFilter::new_or(vec![
            AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Durable),
            AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Ephemeral),
        ]);
        assert_eq!(modes_from_filter(&Some(f)), None);
    }

    #[test]
    fn not_returns_none() {
        let f = AgentFilter::new_not(AgentFilter::new_mode(
            FilterComparator::Equal,
            AgentMode::Durable,
        ));
        assert_eq!(modes_from_filter(&Some(f)), None);
    }

    #[test]
    fn and_with_nested_or_containing_mode_returns_none() {
        // And([ Or([ Mode(Equal, Ephemeral), Name(Equal, "x") ]) ])
        // The Or can be satisfied by Ephemeral agents, so we must not narrow
        // the scan to Durable just because there's no top-level Mode child.
        let f = AgentFilter::new_and(vec![AgentFilter::new_or(vec![
            AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Ephemeral),
            AgentFilter::new_name(StringFilterComparator::Equal, "x".to_string()),
        ])]);
        assert_eq!(modes_from_filter(&Some(f)), None);
    }

    #[test]
    fn and_with_nested_not_containing_mode_returns_none() {
        let f = AgentFilter::new_and(vec![AgentFilter::new_not(AgentFilter::new_mode(
            FilterComparator::Equal,
            AgentMode::Ephemeral,
        ))]);
        assert_eq!(modes_from_filter(&Some(f)), None);
    }

    #[test]
    fn and_with_top_level_mode_eq_and_nested_mode_returns_none() {
        // Top-level Mode(Equal, Durable) plus a sibling Or containing Mode.
        let f = AgentFilter::new_and(vec![
            AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Durable),
            AgentFilter::new_or(vec![
                AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Ephemeral),
                AgentFilter::new_name(StringFilterComparator::Equal, "x".to_string()),
            ]),
        ]);
        assert_eq!(modes_from_filter(&Some(f)), None);
    }

    #[test]
    fn name_only_filter_defaults_to_durable() {
        // A filter that doesn't reference Mode at all defaults to Durable,
        // matching the no-filter behaviour.
        let f = AgentFilter::new_name(StringFilterComparator::Equal, "x".to_string());
        assert_eq!(modes_from_filter(&Some(f)), Some(AgentMode::Durable));
    }

    #[test]
    fn or_without_mode_defaults_to_durable() {
        let f = AgentFilter::new_or(vec![
            AgentFilter::new_name(StringFilterComparator::Equal, "a".to_string()),
            AgentFilter::new_name(StringFilterComparator::Equal, "b".to_string()),
        ]);
        assert_eq!(modes_from_filter(&Some(f)), Some(AgentMode::Durable));
    }

    #[test]
    fn not_without_mode_defaults_to_durable() {
        let f = AgentFilter::new_not(AgentFilter::new_name(
            StringFilterComparator::Equal,
            "x".to_string(),
        ));
        assert_eq!(modes_from_filter(&Some(f)), Some(AgentMode::Durable));
    }
}
