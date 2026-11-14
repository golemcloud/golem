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

use super::*;
use pretty_assertions::assert_eq;
use test_r::test;

#[test]
#[timeout("2m")]
async fn exclusive_create_retries_after_deletion_instead_of_returning_a_missing_agent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, create_worker_response,
    };

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent = executor
        .start_agent(&component.id, agent_id!("Counter", "create-after-delete"))
        .await?;
    let original = executor.get_worker_metadata(&agent).await?.fingerprint;
    let owned = OwnedAgentId::new(context.default_environment_id, &agent);
    executor.set_worker_deletion_hook(Arc::new(DeletionStageHook::new(
        owned.clone(),
        None,
        Some(WorkerDeletionStage::CacheRemoved),
    )));
    assert!(executor.delete_worker(&agent).await.is_err());
    assert!(executor.worker_is_cached(&owned).await);
    assert!(executor.get_worker_metadata_opt(&agent).await?.is_none());

    let key = IdempotencyKey::fresh();
    let request = CreateWorkerRequest {
        agent_id: Some(agent.clone().into()),
        component_owner_account_id: Some(context.account_id.into()),
        environment_id: Some(context.default_environment_id.into()),
        ignore_already_existing: false,
        auth_ctx: Some(executor.auth_ctx().into()),
        idempotency_key: Some(key.clone().into()),
        ..Default::default()
    };
    let blocked = executor
        .client
        .clone()
        .prepare_worker(request.clone())
        .await?
        .into_inner();
    let Some(create_worker_response::Result::Failure(error)) = blocked.result else {
        anyhow::bail!("exclusive acquisition returned the deleted incarnation");
    };
    assert!(matches!(
        WorkerExecutorError::try_from(error).map_err(anyhow::Error::msg)?,
        WorkerExecutorError::ShardingNotReady
    ));

    executor.delete_worker(&agent).await?;
    let response = executor
        .client
        .clone()
        .prepare_worker(request)
        .await?
        .into_inner();
    let Some(create_worker_response::Result::Success(success)) = response.result else {
        anyhow::bail!("exclusive create did not succeed after deletion: {response:?}");
    };
    let returned = success.instance_id.unwrap();
    assert_ne!(returned, original.0.into());
    let metadata = executor.get_worker_metadata(&agent).await?;
    assert_eq!(returned, metadata.fingerprint.0.into());
    Ok(())
}
