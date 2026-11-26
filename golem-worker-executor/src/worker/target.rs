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

//! Resolution of the address of a request to the agent ID that it reaches and the owner that
//! it may create. Every request that names an agent by key or creates one goes through here,
//! so a key always derives the same ID and the same canonical key.

use crate::services::agent_types::AgentTypesService;
use crate::services::component::ComponentService;
use crate::worker::{CreateIntent, NewOwner};
use golem_common::model::agent::{AgentKey, AgentMode, AgentTarget, ParsedAgentKey};
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::type_id::TypeId;
use golem_common::model::{AgentId, IdempotencyKey, OwnedAgentId};
use golem_service_base::error::worker_executor::WorkerExecutorError;

/// The agent that a request reaches, and the owner that it creates when the agent is missing.
#[derive(Clone, Debug)]
pub struct ResolvedTarget {
    pub owned_agent_id: OwnedAgentId,
    /// `None` when the request reaches only an existing agent.
    pub new_owner: Option<NewOwner>,
    /// The mode of the agent type of `new_owner`.
    pub agent_mode: Option<AgentMode>,
}

impl ResolvedTarget {
    /// A target that reaches only the existing agent with this ID.
    pub fn existing(owned_agent_id: OwnedAgentId) -> Self {
        Self {
            owned_agent_id,
            new_owner: None,
            agent_mode: None,
        }
    }

    /// The target, constructor and component that a request to another executor carries, so
    /// that executor derives the same ID and creates the same owner.
    pub fn to_proto(&self) -> Result<ProtoTarget, WorkerExecutorError> {
        use golem_api_grpc::proto::golem::worker::agent_target::Target;
        let id_target = || golem_api_grpc::proto::golem::worker::AgentTarget {
            target: Some(Target::Id(self.owned_agent_id.agent_id.into())),
        };
        let Some(owner) = &self.new_owner else {
            return Ok(ProtoTarget {
                target: id_target(),
                constructor: None,
                component_id: None,
            });
        };
        let constructor = owner
            .constructor
            .clone()
            .map(|constructor| {
                golem_api_grpc::proto::golem::worker::AgentKey::try_from(constructor)
                    .map_err(WorkerExecutorError::invalid_request)
            })
            .transpose()?;
        Ok(match (owner.keyed, constructor) {
            (true, Some(key)) => ProtoTarget {
                target: golem_api_grpc::proto::golem::worker::AgentTarget {
                    target: Some(Target::Key(key)),
                },
                constructor: None,
                component_id: Some(owner.component_id.into()),
            },
            (_, constructor) => ProtoTarget {
                target: id_target(),
                constructor,
                component_id: Some(owner.component_id.into()),
            },
        })
    }

    /// The create intent of an invocation of this target.
    pub fn invocation_intent(
        &self,
        idempotency_key: Option<IdempotencyKey>,
        invocation: Option<golem_api_grpc::proto::golem::worker::InvocationStart>,
    ) -> CreateIntent {
        match &self.new_owner {
            Some(owner) => CreateIntent::Invocation {
                owner: owner.clone(),
                idempotency_key,
                invocation: invocation.map(Box::new),
            },
            None => CreateIntent::Existing,
        }
    }
}

/// The fields that address an agent in a request to another executor or the worker service.
pub struct ProtoTarget {
    pub target: golem_api_grpc::proto::golem::worker::AgentTarget,
    pub constructor: Option<golem_api_grpc::proto::golem::worker::AgentKey>,
    pub component_id: Option<golem_api_grpc::proto::golem::component::ComponentId>,
}

/// A constructor checked against the component that implements its agent type.
struct CheckedConstructor {
    constructor: ParsedAgentKey,
    type_id: TypeId,
    agent_mode: AgentMode,
}

/// Resolves the target of a request. A key target selects the keyed agent whose ID derives
/// from the key, and may create it. An ID target with `constructor` may create an agent without
/// a key at that pre-allocated ID. An ID target without `constructor` reaches only an existing
/// agent. `component_id` is required whenever the request may create.
pub async fn resolve_target(
    component_service: &dyn ComponentService,
    agent_types: &dyn AgentTypesService,
    environment_id: EnvironmentId,
    target: AgentTarget,
    constructor: Option<ParsedAgentKey>,
    component_id: Option<ComponentId>,
) -> Result<ResolvedTarget, WorkerExecutorError> {
    match (target, constructor) {
        (AgentTarget::Key(_), Some(_)) => Err(WorkerExecutorError::invalid_request(
            "A key target has no separate constructor",
        )),
        (AgentTarget::Key(key), None) => {
            let component_id = required_component(component_id)?;
            resolve_key(
                component_service,
                agent_types,
                environment_id,
                component_id,
                key.into(),
            )
            .await
        }
        (AgentTarget::Id(agent_id), Some(constructor)) => {
            let component_id = required_component(component_id)?;
            resolve_unkeyed(
                component_service,
                agent_types,
                environment_id,
                component_id,
                agent_id,
                constructor,
            )
            .await
        }
        (AgentTarget::Id(agent_id), None) => Ok(ResolvedTarget {
            owned_agent_id: OwnedAgentId::new(environment_id, &agent_id),
            new_owner: None,
            agent_mode: None,
        }),
    }
}

/// Resolves a key to the keyed agent that it selects. The key is canonicalized against the
/// constructor schema of the implementing component, not the schema that the caller sent, so
/// two spellings of one key reach one agent.
pub async fn resolve_key(
    component_service: &dyn ComponentService,
    agent_types: &dyn AgentTypesService,
    environment_id: EnvironmentId,
    component_id: ComponentId,
    key: ParsedAgentKey,
) -> Result<ResolvedTarget, WorkerExecutorError> {
    let checked = check_constructor(
        component_service,
        agent_types,
        environment_id,
        component_id,
        key,
    )
    .await?;
    if checked.agent_mode != AgentMode::Durable {
        return Err(WorkerExecutorError::invalid_request(
            "An ephemeral agent has no key: address it by ID",
        ));
    }
    let (agent_type, parameters) = (
        checked.constructor.agent_type.clone(),
        checked.constructor.parameters.clone(),
    );
    let key = AgentKey::new(agent_type, parameters)
        .map_err(|err| WorkerExecutorError::invalid_request(err.to_string()))?;
    let agent_id = AgentId::keyed(environment_id, checked.type_id, &key);
    Ok(ResolvedTarget {
        owned_agent_id: OwnedAgentId::new(environment_id, &agent_id),
        new_owner: Some(NewOwner::keyed(component_id, checked.constructor)),
        agent_mode: Some(checked.agent_mode),
    })
}

/// Resolves the creation of an agent without a key at a pre-allocated ID. The ID must be one
/// that Golem issues, with the tag of the constructor's agent type.
pub async fn resolve_unkeyed(
    component_service: &dyn ComponentService,
    agent_types: &dyn AgentTypesService,
    environment_id: EnvironmentId,
    component_id: ComponentId,
    agent_id: AgentId,
    constructor: ParsedAgentKey,
) -> Result<ResolvedTarget, WorkerExecutorError> {
    if !agent_id.is_issued() {
        return Err(WorkerExecutorError::invalid_request(
            "The agent ID is not one that Golem issues",
        ));
    }
    let checked = check_constructor(
        component_service,
        agent_types,
        environment_id,
        component_id,
        constructor,
    )
    .await?;
    if agent_id.type_id() != Some(checked.type_id) {
        return Err(WorkerExecutorError::invalid_request(
            "The agent ID does not carry the type tag of the constructor's agent type",
        ));
    }
    Ok(ResolvedTarget {
        owned_agent_id: OwnedAgentId::new(environment_id, &agent_id),
        new_owner: Some(NewOwner::unkeyed(component_id, checked.constructor)),
        agent_mode: Some(checked.agent_mode),
    })
}

/// The keyed agent of `constructor`, from a constructor that the caller decoded against the
/// deployed agent type with type ID `type_id`, which `component_id` implements. The host uses
/// this instead of [`resolve_key`] because it derives the ID from recorded data alone, so replay
/// derives the same ID without a registry lookup.
pub fn keyed_target(
    environment_id: EnvironmentId,
    type_id: TypeId,
    component_id: ComponentId,
    constructor: ParsedAgentKey,
) -> Result<ResolvedTarget, WorkerExecutorError> {
    let key = AgentKey::new(
        constructor.agent_type.clone(),
        constructor.parameters.clone(),
    )
    .map_err(|err| WorkerExecutorError::invalid_request(err.to_string()))?;
    let agent_id = AgentId::keyed(environment_id, type_id, &key);
    Ok(ResolvedTarget {
        owned_agent_id: OwnedAgentId::new(environment_id, &agent_id),
        new_owner: Some(NewOwner::keyed(component_id, constructor)),
        agent_mode: Some(AgentMode::Durable),
    })
}

/// A new agent without a key at `agent_id`, which the host allocated for the constructor's
/// agent type, so it carries that type's tag.
pub fn unkeyed_target(
    environment_id: EnvironmentId,
    agent_id: AgentId,
    component_id: ComponentId,
    constructor: ParsedAgentKey,
    agent_mode: AgentMode,
) -> ResolvedTarget {
    ResolvedTarget {
        owned_agent_id: OwnedAgentId::new(environment_id, &agent_id),
        new_owner: Some(NewOwner::unkeyed(component_id, constructor)),
        agent_mode: Some(agent_mode),
    }
}

/// The one-shot instance of an ephemeral agent type that runs the invocation with owner key
/// `owner_key`, so a retry of the invocation reaches the same instance.
pub fn ephemeral_invocation_target(
    environment_id: EnvironmentId,
    type_id: TypeId,
    component_id: ComponentId,
    constructor: ParsedAgentKey,
    owner_key: &IdempotencyKey,
) -> ResolvedTarget {
    unkeyed_target(
        environment_id,
        AgentId::ephemeral_invocation(environment_id, type_id, owner_key),
        component_id,
        constructor,
        AgentMode::Ephemeral,
    )
}

async fn check_constructor(
    component_service: &dyn ComponentService,
    agent_types: &dyn AgentTypesService,
    environment_id: EnvironmentId,
    component_id: ComponentId,
    constructor: ParsedAgentKey,
) -> Result<CheckedConstructor, WorkerExecutorError> {
    let component = component_service.get_metadata(component_id, None).await?;
    if component.environment_id != environment_id {
        return Err(WorkerExecutorError::invalid_request(
            "The component does not belong to the agent's environment",
        ));
    }
    let agent_type = component
        .metadata
        .find_agent_type_by_name_ref(&constructor.agent_type)
        .ok_or_else(|| {
            WorkerExecutorError::invalid_request(format!(
                "The component does not implement agent type {}",
                constructor.agent_type
            ))
        })?;
    let agent_mode = agent_type.mode;
    let constructor = constructor
        .canonical_constructor(&component.metadata)
        .map_err(WorkerExecutorError::invalid_request)?;
    let registered = agent_types
        .get(
            environment_id,
            component_id,
            component.revision,
            &constructor.agent_type,
        )
        .await?
        .ok_or_else(|| {
            WorkerExecutorError::invalid_request(format!(
                "Agent type {} is not deployed",
                constructor.agent_type
            ))
        })?;
    Ok(CheckedConstructor {
        constructor,
        type_id: registered.type_id,
        agent_mode,
    })
}

fn required_component(
    component_id: Option<ComponentId>,
) -> Result<ComponentId, WorkerExecutorError> {
    component_id.ok_or_else(|| {
        WorkerExecutorError::invalid_request("component_id is required for a request that creates")
    })
}
