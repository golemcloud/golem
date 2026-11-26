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
use golem_api_grpc::proto::golem::worker::v1::worker_execution_error;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    InvocationHandleOperation as Operation, InvocationHandleRequest,
    invocation_handle_response as response, invocation_handle_success as success,
};
use golem_common::model::OwnedAgentId;
use golem_common::model::account::AccountEmail;
use golem_common::model::application::ApplicationName;
use golem_common::model::card::owner::{AgentOwnerLeafPattern, AgentOwnerPattern};
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{
    AgentInvocationIdPattern, AgentResourcePattern, AgentVerb, ClassPermissionPattern,
    ClassPermissionTarget, EffectiveSurface, PermissionPattern, PermissionTarget,
};
use golem_common::model::component::{ComponentId, ComponentName};
use golem_common::model::environment::EnvironmentName;
use golem_worker_executor::services::direct_invocation_auth::{
    DirectInvocationAuthService, EnvironmentOwnerAccountId,
};
use golem_worker_executor::services::rpc::RpcError;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::time::Duration;
use test_r::test;

fn owner(target: &OwnedAgentId, agent_type: Option<AgentTypeName>) -> AgentOwnerPattern {
    AgentOwnerPattern::Agent {
        account: AccountEmail::new("handle-authority@test".to_string()),
        application: ApplicationName("handle-authority".to_string()),
        environment: EnvironmentName("test".to_string()),
        component: ComponentName("agent_rpc_rust".to_string()),
        agent: AgentOwnerLeafPattern::agent(target.agent_id, agent_type),
    }
}

// The executor override has no registry argument. Substitute only the registry's owner
// namespace here; evaluate the actual verb, resource and AuthCtx with production machinery.
// This does not exercise registry metadata lookup, environment validation or its cache.
struct SurfaceManagementAuth;

#[async_trait::async_trait]
impl DirectInvocationAuthService for SurfaceManagementAuth {
    async fn check(
        &self,
        account: AccountId,
        target: &OwnedAgentId,
        _: ComponentId,
        agent_type: Option<AgentTypeName>,
        verb: AgentVerb,
        resource: AgentResourcePattern,
        auth: &AuthCtx,
    ) -> Result<EnvironmentOwnerAccountId, RpcError> {
        auth.authorize_permission(&PermissionTarget::Agent(ClassPermissionTarget {
            owner: owner(target, agent_type),
            verb: Some(verb),
            resource,
        }))
        .map_err(|error| RpcError::Denied {
            details: error.to_string(),
        })?;
        Ok(EnvironmentOwnerAccountId(account))
    }
}

fn surface_auth(
    account: AccountId,
    target: &OwnedAgentId,
    grants: &[(AgentVerb, InvocationId)],
    denials: &[(AgentVerb, InvocationId)],
) -> AuthCtx {
    let pattern = |&(verb, key): &(AgentVerb, InvocationId)| {
        PermissionPattern::Agent(ClassPermissionPattern {
            owner: owner(
                target,
                Some(AgentTypeName("ScheduledInvocationServer".to_string())),
            ),
            verb: Some(verb),
            resource: AgentResourcePattern::InvocationId(AgentInvocationIdPattern::Uuid(key.0)),
            recipient: RecipientPattern::Any,
        })
    };
    AuthCtx::agent_with_effective_surface(
        account,
        AccountEmail::new("handle-caller@test".to_string()),
        EffectiveSurface::from_grants(
            &grants.iter().map(pattern).collect::<Vec<_>>(),
            &denials.iter().map(pattern).collect::<Vec<_>>(),
            &[],
            &[],
            &RecipientPattern::Any,
        )
        .unwrap(),
    )
}

async fn operate(
    executor: &TestWorkerExecutor,
    target: &OwnedAgentId,
    caller: &OwnedAgentId,
    key: InvocationId,
    operation: Operation,
    auth: &AuthCtx,
) -> anyhow::Result<response::Result> {
    let caller = super::agent_principal(executor, caller).await?;
    Ok(executor
        .client
        .clone()
        .invocation_handle(InvocationHandleRequest {
            agent_id: Some(target.agent_id.into()),
            environment_id: Some(target.environment_id.into()),
            invocation_id: Some(key.0.into()),
            caller: Some(caller.into()),
            auth_ctx: Some(auth.clone().into()),
            operation: operation as i32,
        })
        .await?
        .into_inner()
        .result
        .expect("a handle operation result"))
}

fn assert_denied(result: response::Result) {
    assert!(
        matches!(&result, response::Result::Failure(error)
            if matches!(error.error, Some(worker_execution_error::Error::PermissionDenied(_)))),
        "expected permission denied, got {result:?}"
    );
}

fn assert_success(result: response::Result, expected: success::Result) {
    assert!(
        matches!(&result, response::Result::Success(value) if value.result.as_ref() == Some(&expected)),
        "expected {expected:?}, got {result:?}"
    );
}

async fn setup(
    context: &TestContext,
    deps: &WorkerExecutorTestDependencies,
    fixture: &PrecompiledComponent,
) -> anyhow::Result<(
    TestWorkerExecutor,
    OwnedAgentId,
    OwnedAgentId,
    OwnedAgentId,
    InvocationId,
    InvocationId,
)> {
    let executor = start_with_overrides(
        deps,
        context,
        TestExecutorOverrides {
            create_direct_invocation_auth: Some(Arc::new(|| Arc::new(SurfaceManagementAuth))),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let server = agent_key!("ScheduledInvocationServer", "surface-target");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let starter = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "surface-starter"),
        )
        .await?;
    let other = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "surface-other"),
        )
        .await?;
    executor
        .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
        .await?;
    let key = InvocationId(uuid::Uuid::new_v4());
    let other_key = InvocationId(uuid::Uuid::new_v4());
    let worker = executor.active_agent(&target).await.unwrap().primary();
    for (caller, invocation_id) in [(&starter, key), (&other, other_key)] {
        let fingerprint = executor
            .active_agent(caller)
            .await
            .unwrap()
            .primary()
            .get_initial_worker_metadata()
            .fingerprint;
        worker
            .schedule_invocation(
                AgentInvocation::AgentMethod {
                    idempotency_key: invocation_id,
                    method_name: "inc_global_by".to_string(),
                    input: data_value!(17u64).into_parts().1.try_into().unwrap(),
                    allow_streams: false,
                    invocation_context: InvocationContextStack::fresh(),
                    caller: callers::agent_in(
                        caller.agent_id,
                        AgentTypeName("ScheduledInvocationClient".to_string()),
                        fingerprint,
                        caller.environment_id,
                    ),
                    scope_card: None,
                },
                jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1),
            )
            .await?;
    }
    drop(worker);
    Ok((executor, target, starter, other, key, other_key))
}

#[test]
#[timeout("4m")]
async fn invocation_handle_management_surfaces_separate_view_cancel_and_invocation_resources(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let (executor, target, starter, other, key, other_key) = setup(&context, deps, fixture).await?;
    let view = surface_auth(context.account_id, &target, &[(AgentVerb::View, key)], &[]);
    let cancel = surface_auth(
        context.account_id,
        &target,
        &[(AgentVerb::CancelInvocation, key)],
        &[],
    );
    let both = [(AgentVerb::View, key), (AgentVerb::CancelInvocation, key)];
    let empty = surface_auth(context.account_id, &target, &[], &[]);
    let denied = surface_auth(context.account_id, &target, &both, &both);
    let wrong_resource = surface_auth(
        context.account_id,
        &target,
        &[
            (AgentVerb::View, other_key),
            (AgentVerb::CancelInvocation, other_key),
        ],
        &[],
    );
    for auth in [&empty, &denied, &wrong_resource] {
        for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
            assert_denied(operate(&executor, &target, &other, key, operation, auth).await?);
        }
    }
    // These grants are usable, but only for the other invocation. Its starter is `other`,
    // so using `starter` here cannot pass through persisted-starter authority.
    assert_success(
        operate(
            &executor,
            &target,
            &starter,
            other_key,
            Operation::Status,
            &wrong_resource,
        )
        .await?,
        success::Result::Status(InvocationLifecycleState::Scheduled as i32),
    );
    assert_success(
        operate(
            &executor,
            &target,
            &starter,
            other_key,
            Operation::Cancel,
            &wrong_resource,
        )
        .await?,
        success::Result::Cancelled(true),
    );
    assert_success(
        operate(&executor, &target, &other, key, Operation::Status, &view).await?,
        success::Result::Status(InvocationLifecycleState::Scheduled as i32),
    );
    assert_denied(operate(&executor, &target, &other, key, Operation::Cancel, &view).await?);
    for operation in [Operation::Status, Operation::Await] {
        assert_denied(operate(&executor, &target, &other, key, operation, &cancel).await?);
    }
    let observing = operate(&executor, &target, &other, key, Operation::Await, &view);
    tokio::pin!(observing);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut observing)
            .await
            .is_err(),
        "View must authorize Await without requiring CancelInvocation"
    );
    assert_success(
        operate(&executor, &target, &other, key, Operation::Cancel, &cancel).await?,
        success::Result::Cancelled(true),
    );
    let terminal = tokio::time::timeout(Duration::from_secs(10), observing).await??;
    assert!(matches!(&terminal, response::Result::Success(value)
        if matches!(value.result, Some(success::Result::InvocationFailure(_)))));
    assert_eq!(
        operate(&executor, &target, &other, key, Operation::Await, &view).await?,
        terminal
    );
    assert_success(
        operate(&executor, &target, &other, key, Operation::Status, &view).await?,
        success::Result::Status(InvocationLifecycleState::Cancelled as i32),
    );
    assert_denied(operate(&executor, &target, &other, key, Operation::Await, &cancel).await?);
    assert_denied(operate(&executor, &target, &other, key, Operation::Cancel, &view).await?);

    // Absence and a terminal no-op must not disclose information before authorization.
    let missing = InvocationId(uuid::Uuid::new_v4());
    for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
        assert_denied(operate(&executor, &target, &other, key, operation, &empty).await?);
        assert_denied(operate(&executor, &target, &other, missing, operation, &empty).await?);
        let verb = if operation == Operation::Cancel {
            AgentVerb::CancelInvocation
        } else {
            AgentVerb::View
        };
        let allowed_missing = surface_auth(context.account_id, &target, &[(verb, missing)], &[]);
        assert!(matches!(
            operate(&executor, &target, &other, missing, operation, &allowed_missing).await?,
            response::Result::Success(value) if matches!(value.result, Some(success::Result::NotFound(_)))
        ));
    }
    Ok(())
}

#[test]
#[timeout("4m")]
async fn invocation_handle_starter_authority_is_invocation_only_with_an_empty_agent_surface(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let (executor, target, starter, other, key, other_key) = setup(&context, deps, fixture).await?;
    let empty = surface_auth(context.account_id, &target, &[], &[]);
    for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
        assert_denied(operate(&executor, &target, &other, key, operation, &empty).await?);
        for foreign in [other_key, InvocationId(uuid::Uuid::new_v4())] {
            assert_denied(operate(&executor, &target, &starter, foreign, operation, &empty).await?);
        }
    }
    assert_success(
        operate(&executor, &target, &starter, key, Operation::Status, &empty).await?,
        success::Result::Status(InvocationLifecycleState::Scheduled as i32),
    );
    let observing = operate(&executor, &target, &starter, key, Operation::Await, &empty);
    tokio::pin!(observing);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut observing)
            .await
            .is_err(),
        "the persisted starter needs no management surface to await its invocation"
    );
    assert_success(
        operate(&executor, &target, &starter, key, Operation::Cancel, &empty).await?,
        success::Result::Cancelled(true),
    );
    let terminal = tokio::time::timeout(Duration::from_secs(10), observing).await??;
    assert!(matches!(terminal, response::Result::Success(value)
        if matches!(value.result, Some(success::Result::InvocationFailure(_)))));
    assert_success(
        operate(&executor, &target, &starter, key, Operation::Cancel, &empty).await?,
        success::Result::Cancelled(false),
    );
    Ok(())
}

#[test]
#[timeout("2m")]
async fn handle_missing_owner_does_not_bypass_authorization(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let (executor, target, _starter, other, key, _other_key) =
        setup(&context, deps, fixture).await?;
    let empty = surface_auth(context.account_id, &target, &[], &[]);
    for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
        assert_denied(operate(&executor, &target, &other, key, operation, &empty).await?);
    }
    executor.delete_worker(&target).await?;
    let mut disclosures = Vec::new();
    for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
        let result = operate(&executor, &target, &other, key, operation, &empty).await?;
        if !matches!(&result, response::Result::Failure(error)
            if matches!(error.error, Some(worker_execution_error::Error::PermissionDenied(_))))
        {
            disclosures.push((operation, result));
        }
    }
    assert!(
        disclosures.is_empty(),
        "an unrelated caller with no grants must not distinguish deletion from denial: {disclosures:?}"
    );
    let manager = surface_auth(
        context.account_id,
        &target,
        &[(AgentVerb::View, key), (AgentVerb::CancelInvocation, key)],
        &[],
    );
    for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
        assert!(
            matches!(operate(&executor, &target, &other, key, operation, &manager).await?,
            response::Result::Success(value)
                if matches!(value.result, Some(success::Result::NotFound(_))))
        );
    }
    assert!(!executor.worker_is_cached(&target).await);
    assert!(executor.get_worker_metadata(&target).await.is_err());
    Ok(())
}

/// A handle request that does not come from an authenticated internal caller is an invalid
/// request, whether or not its owner exists, before anything else is checked.
#[test]
#[timeout("2m")]
async fn a_handle_request_from_a_user_is_an_invalid_request(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let (executor, target, starter, _other, key, _other_key) =
        setup(&context, deps, fixture).await?;
    let user = executor.auth_ctx();
    assert!(matches!(user, AuthCtx::User(_)));
    let missing = OwnedAgentId::new(
        target.environment_id,
        &golem_common::model::AgentId::random(
            target
                .agent_id
                .type_id()
                .expect("the target ID names its type"),
        ),
    );
    for owner in [&target, &missing] {
        for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
            let result = operate(&executor, owner, &starter, key, operation, &user).await?;
            assert!(
                matches!(&result, response::Result::Failure(error)
                    if matches!(error.error, Some(worker_execution_error::Error::InvalidRequest(_)))),
                "{operation:?} on {owner:?}: {result:?}"
            );
        }
    }
    Ok(())
}
