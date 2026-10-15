use super::*;
use support::*;

mod host;
mod lifecycle;
mod metering;
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
        tokio::spawn(async move { limits.run_batch_for_test().await })
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        while limits.initialized_policy_revision_for_test(account).await != Some(revision) {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(refresh)
}
