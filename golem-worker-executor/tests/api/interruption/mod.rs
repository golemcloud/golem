use super::*;
use crate::resource_limit_observations::ResourceEntryObservations;
use golem_worker_executor::durable_host::tool::ToolOwnerFailureMetadata;
use periodic_limits::PeriodicResourceLimits as ResourceLimitsGrpc;
use support::*;

mod host;
mod lifecycle;
mod metering;
pub(super) mod periodic_limits;
mod selection;
mod support;

test_r::tag_suite!(selection, group5);

pub(super) async fn applied_refresh(
    limits: &Arc<ResourceLimitsGrpc>,
    registry: &MutableResourceLimitsRegistry,
    account: AccountId,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let revision = registry.current_limits().monthly_policy_revision;
    let refresh = {
        let limits = limits.clone();
        tokio::spawn(async move { limits.release_registry_reply().await })
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        while limits.initialized_policy_revision_for_test(account).await != Some(revision) {
            tokio::task::yield_now().await;
        }
        while !refresh.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(refresh)
}
