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

//! Agent addressing helpers that the test DSL does not cover: looking a keyed agent up
//! without creating it, creating an agent without a key, invoking an agent by ID, and
//! deriving the ID of a keyed agent before it exists.

use anyhow::anyhow;
use futures_concurrency::future::Join;
use golem_client::api::{AgentClient, AgentError, RegistryServiceClient};
use golem_client::model::{
    AgentConfigEntryDto, AgentInvocationMode, AgentInvocationRequest, AgentKeyRequest,
    CreateAgentRequest,
};
use golem_common::base_model::type_id::TypeId;
use golem_common::model::agent::{AgentKey, AgentTypeName, OwnedAgentId, ParsedAgentKey};
use golem_common::model::component::{ComponentDto, ComponentId};
use golem_common::model::{AgentId, IdempotencyKey};
use golem_common::schema::{ExternalSchemaValue, TypedSchemaValue};
use golem_test_framework::config::TestDependencies;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::dsl::{AgentResult, TestDsl, TestDslExtended};
use std::collections::HashMap;

fn external_parameters(key: &ParsedAgentKey) -> anyhow::Result<ExternalSchemaValue> {
    ExternalSchemaValue::try_from(key.parameters.value().clone()).map_err(anyhow::Error::msg)
}

/// The ID of the keyed agent of `key` in the component's environment, or an error when the
/// agent does not exist. The lookup does not create the agent.
pub async fn lookup_keyed_agent<Deps: TestDependencies>(
    user: &TestUserContext<Deps>,
    component: &ComponentDto,
    key: &ParsedAgentKey,
) -> anyhow::Result<OwnedAgentId> {
    let reference = user
        .deps
        .worker_service()
        .agent_http_client(&user.token)
        .await
        .lookup_agent(
            &component.environment_id.0,
            &key.agent_type.0,
            &AgentKeyRequest {
                parameters: external_parameters(key)?,
            },
        )
        .await?;
    Ok(OwnedAgentId::new(
        component.environment_id,
        &AgentId(reference.agent_id),
    ))
}

/// The IDs of the keyed agents of `keys` that exist in the component's environment.
pub async fn lookup_keyed_agents<Deps: TestDependencies>(
    user: &TestUserContext<Deps>,
    component: &ComponentDto,
    keys: &[ParsedAgentKey],
) -> Vec<OwnedAgentId> {
    keys.iter()
        .map(|key| lookup_keyed_agent(user, component, key))
        .collect::<Vec<_>>()
        .join()
        .await
        .into_iter()
        .filter_map(|result| result.ok())
        .collect()
}

/// Creates a new agent without a key from the constructor parameters of `constructor`. Each
/// call creates another agent with a fresh ID, which is reachable only by that ID.
pub async fn create_agent<Deps: TestDependencies>(
    user: &TestUserContext<Deps>,
    component: &ComponentDto,
    constructor: &ParsedAgentKey,
) -> anyhow::Result<OwnedAgentId> {
    create_agent_with(user, component, constructor, HashMap::new(), Vec::new()).await
}

/// [`create_agent`] with environment variable and config overrides.
pub async fn create_agent_with<Deps: TestDependencies>(
    user: &TestUserContext<Deps>,
    component: &ComponentDto,
    constructor: &ParsedAgentKey,
    env: HashMap<String, String>,
    config: Vec<AgentConfigEntryDto>,
) -> anyhow::Result<OwnedAgentId> {
    Ok(try_create_agent(user, component, constructor, false, env, config).await??)
}

/// Creates an agent from the constructor parameters of `constructor`. With `keyed` it creates
/// the keyed agent of `constructor`, which fails with a conflict when that agent exists;
/// without it, it creates a new agent without a key. The inner result is the response of the
/// create request.
pub async fn try_create_agent<Deps: TestDependencies>(
    user: &TestUserContext<Deps>,
    component: &ComponentDto,
    constructor: &ParsedAgentKey,
    keyed: bool,
    env: HashMap<String, String>,
    config: Vec<AgentConfigEntryDto>,
) -> anyhow::Result<Result<OwnedAgentId, golem_client::Error<AgentError>>> {
    let parameters = external_parameters(constructor)?;
    let response = user
        .deps
        .worker_service()
        .agent_http_client(&user.token)
        .await
        .create_agent(
            &component.environment_id.0,
            &constructor.agent_type.0,
            None,
            &CreateAgentRequest {
                parameters,
                config: Some(config),
                env: Some(env),
                keyed: Some(keyed),
            },
        )
        .await;
    Ok(response
        .map(|reference| OwnedAgentId::new(component.environment_id, &AgentId(reference.agent_id))))
}

/// Invokes `method_name` on the agent of `agent_type` that has the ID `agent_id`, and awaits
/// its result.
pub async fn invoke_and_await_agent_by_id<Deps: TestDependencies>(
    user: &TestUserContext<Deps>,
    agent_type: &AgentTypeName,
    agent_id: &OwnedAgentId,
    method_name: &str,
    params: TypedSchemaValue,
) -> anyhow::Result<AgentResult> {
    let result = invoke_agent_by_id_impl(
        user,
        agent_type,
        agent_id,
        &IdempotencyKey::fresh(),
        AgentInvocationMode::Await,
        method_name,
        params,
    )
    .await?;
    let value = result
        .result
        .map(|typed_output| typed_output.into_inner().into_parts().1);
    Ok(AgentResult::new(
        value,
        OwnedAgentId::new(agent_id.environment_id, &AgentId(result.agent_id)),
    ))
}

/// Schedules `method_name` on the agent of `agent_type` that has the ID `agent_id`, without
/// awaiting its result.
pub async fn invoke_agent_by_id<Deps: TestDependencies>(
    user: &TestUserContext<Deps>,
    agent_type: &AgentTypeName,
    agent_id: &OwnedAgentId,
    method_name: &str,
    params: TypedSchemaValue,
) -> anyhow::Result<()> {
    invoke_agent_by_id_impl(
        user,
        agent_type,
        agent_id,
        &IdempotencyKey::fresh(),
        AgentInvocationMode::Schedule,
        method_name,
        params,
    )
    .await?;
    Ok(())
}

async fn invoke_agent_by_id_impl<Deps: TestDependencies>(
    user: &TestUserContext<Deps>,
    agent_type: &AgentTypeName,
    agent_id: &OwnedAgentId,
    idempotency_key: &IdempotencyKey,
    mode: AgentInvocationMode,
    method_name: &str,
    params: TypedSchemaValue,
) -> anyhow::Result<golem_client::model::AgentInvocationResult> {
    let method_parameters =
        ExternalSchemaValue::try_from(params.into_parts().1).map_err(anyhow::Error::msg)?;
    Ok(user
        .deps
        .worker_service()
        .agent_http_client(&user.token)
        .await
        .invoke_agent(
            &agent_id.environment_id.0,
            &agent_type.0,
            Some(&idempotency_key.value),
            &AgentInvocationRequest {
                agent_id: Some(agent_id.agent_id.0),
                parameters: None,
                method: method_name.to_string(),
                method_parameters,
                mode,
                schedule_at: None,
                expected_fingerprint: None,
            },
        )
        .await?)
}

/// The registry-assigned type ID of `agent_type` in the component's environment. The registry
/// assigns it when a deployment registers the agent type, so this reads the environment's
/// deployed agent types rather than the component metadata.
pub async fn agent_type_id<Deps: TestDependencies>(
    user: &TestUserContext<Deps>,
    component: &ComponentDto,
    agent_type: &AgentTypeName,
) -> anyhow::Result<TypeId> {
    let client = user.registry_service_client().await;
    let deployment = client
        .get_environment(&component.environment_id.0)
        .await?
        .current_deployment
        .ok_or_else(|| anyhow!("Environment {} has no deployment", component.environment_id))?;
    client
        .list_deployment_agent_types(
            &component.environment_id.0,
            deployment.deployment_revision.into(),
        )
        .await?
        .values
        .into_iter()
        .find(|candidate| &candidate.agent_type.type_name == agent_type)
        .map(|candidate| candidate.type_id)
        .ok_or_else(|| anyhow!("Agent type {agent_type} has no registry-assigned type ID"))
}

/// The ID that the keyed agent of `key` has in the component's environment, derived as the
/// platform derives it, whether or not the agent exists. `type_id` must be the type ID of
/// `key.agent_type`.
pub fn keyed_agent_id(
    component: &ComponentDto,
    type_id: TypeId,
    key: &ParsedAgentKey,
) -> anyhow::Result<OwnedAgentId> {
    let key = AgentKey::new(key.agent_type.clone(), key.parameters.clone())
        .map_err(|err| anyhow!("Invalid agent key {key}: {err}"))?;
    Ok(OwnedAgentId::new(
        component.environment_id,
        &AgentId::keyed(component.environment_id, type_id, &key),
    ))
}

/// The ID that the keyed agent of `key` has in the environment of the component, whether or
/// not the agent exists. It reads the type ID from the environment's deployed agent types.
pub async fn derive_keyed_agent_id<Deps: TestDependencies>(
    user: &TestUserContext<Deps>,
    component_id: &ComponentId,
    key: &ParsedAgentKey,
) -> anyhow::Result<OwnedAgentId> {
    let component = user.get_latest_component_revision(component_id).await?;
    let type_id = agent_type_id(user, &component, &key.agent_type).await?;
    keyed_agent_id(&component, type_id, key)
}
