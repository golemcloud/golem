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

use super::{WorkerResult, WorkerServiceError};
use golem_api_grpc::proto::golem::worker::InvocationStart;
use golem_api_grpc::proto::golem::worker::agent_target::Target;
use golem_api_grpc::proto::golem::worker::{
    AgentKey as ProtoAgentKey, AgentTarget as ProtoAgentTarget,
};
use golem_common::model::AgentId;
use golem_common::model::agent::{AgentKey, AgentTypeName, ParsedAgentKey};
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::type_id::TypeId;
use golem_service_base::model::component::Component;

/// How a request reaches an agent: the agent ID that routes it to an executor, and the
/// address that the executor resolves again. The executor derives the ID of a key target
/// itself and rejects the request when the ID is not in its shards, so the routing ID
/// must be the ID that the executor derives.
#[derive(Clone, Debug)]
pub struct RoutedTarget {
    pub agent_id: AgentId,
    pub agent_type: Option<AgentTypeName>,
    target: ProtoAgentTarget,
    constructor: Option<ProtoAgentKey>,
    component_id: Option<ComponentId>,
}

impl RoutedTarget {
    /// An existing agent. A request to this target never creates the agent.
    pub fn existing(agent_id: AgentId, agent_type: Option<AgentTypeName>) -> Self {
        Self {
            agent_id,
            agent_type,
            target: id_target(agent_id),
            constructor: None,
            component_id: None,
        }
    }

    /// The keyed agent of `constructor`, which `component` implements as the deployed agent
    /// type `type_id`. The key is canonicalized against the component's constructor schema,
    /// as the executor does.
    pub fn keyed(
        environment_id: EnvironmentId,
        type_id: TypeId,
        component: &Component,
        constructor: &ParsedAgentKey,
    ) -> WorkerResult<Self> {
        let key = canonical_key(component, constructor)?;
        let agent_id = AgentId::keyed(environment_id, type_id, &key);
        Ok(Self {
            agent_id,
            agent_type: Some(key.agent_type().clone()),
            target: ProtoAgentTarget {
                target: Some(Target::Key(proto_key(key)?)),
            },
            constructor: None,
            component_id: Some(component.id),
        })
    }

    /// The agent without a key at the pre-allocated `agent_id`, created from `constructor`
    /// when it is missing.
    pub fn unkeyed(
        agent_id: AgentId,
        component: &Component,
        constructor: &ParsedAgentKey,
    ) -> WorkerResult<Self> {
        let constructor = constructor
            .canonical_constructor(&component.metadata)
            .map_err(WorkerServiceError::TypeChecker)?;
        Ok(Self {
            agent_id,
            agent_type: Some(constructor.agent_type.clone()),
            target: id_target(agent_id),
            constructor: Some(
                constructor
                    .try_into()
                    .map_err(WorkerServiceError::TypeChecker)?,
            ),
            component_id: Some(component.id),
        })
    }

    /// The fresh owner of a native tool that is called on `component_id` instead of on an
    /// agent. The executor creates it.
    pub fn fresh_tool_owner(agent_id: AgentId, component_id: ComponentId) -> Self {
        Self {
            agent_id,
            agent_type: None,
            target: id_target(agent_id),
            constructor: None,
            component_id: Some(component_id),
        }
    }

    /// Whether a request to this target may create the agent.
    pub fn creates(&self) -> bool {
        self.component_id.is_some()
    }

    pub fn component_id(&self) -> Option<ComponentId> {
        self.component_id
    }

    /// The fields of a create request: the constructor, the pre-allocated ID of an agent
    /// without a key, and the implementing component.
    pub fn create_fields(
        &self,
    ) -> WorkerResult<(
        ProtoAgentKey,
        Option<golem_api_grpc::proto::golem::worker::AgentId>,
        ComponentId,
    )> {
        let component_id = self.component_id.ok_or_else(|| {
            WorkerServiceError::Internal("a create request has no component".to_string())
        })?;
        match (&self.target.target, &self.constructor) {
            (Some(Target::Key(key)), None) => Ok((key.clone(), None, component_id)),
            (Some(Target::Id(id)), Some(constructor)) => {
                Ok((constructor.clone(), Some(*id), component_id))
            }
            _ => Err(WorkerServiceError::Internal(
                "a create request names an agent without its constructor".to_string(),
            )),
        }
    }

    /// Sets the target fields of an invocation start.
    pub fn apply(&self, start: &mut InvocationStart) {
        start.target = Some(self.target.clone());
        start.constructor = self.constructor.clone();
        start.component_id = self.component_id.map(Into::into);
    }

    /// The target fields of a worker-service invocation request.
    pub fn invoke_fields(
        &self,
    ) -> (
        ProtoAgentTarget,
        Option<ProtoAgentKey>,
        Option<golem_api_grpc::proto::golem::component::ComponentId>,
    ) {
        (
            self.target.clone(),
            self.constructor.clone(),
            self.component_id.map(Into::into),
        )
    }
}

fn id_target(agent_id: AgentId) -> ProtoAgentTarget {
    ProtoAgentTarget {
        target: Some(Target::Id(agent_id.into())),
    }
}

fn canonical_key(component: &Component, constructor: &ParsedAgentKey) -> WorkerResult<AgentKey> {
    constructor
        .canonical_key(&component.metadata)
        .map_err(WorkerServiceError::TypeChecker)
}

fn proto_key(key: AgentKey) -> WorkerResult<ProtoAgentKey> {
    ProtoAgentKey::try_from(key).map_err(WorkerServiceError::TypeChecker)
}
