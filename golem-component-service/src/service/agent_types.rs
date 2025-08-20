// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::error::ComponentError;
use crate::model::agent_types::RegisteredAgentType;
use crate::service::component::ComponentService;
use async_trait::async_trait;
use dashmap::DashMap;
use golem_common::model::component::ComponentOwner;
use golem_common::model::ProjectId;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::Arc;

#[async_trait]
pub trait AgentTypesService: Debug + Send + Sync {
    async fn get_all_agent_types(
        &self,
        owner: &ComponentOwner,
    ) -> Result<Vec<RegisteredAgentType>, ComponentError>;

    async fn get_agent_type(
        &self,
        agent_type: &str,
        owner: &ComponentOwner,
    ) -> Result<Option<RegisteredAgentType>, ComponentError> {
        let all_agent_types = self.get_all_agent_types(owner).await?;
        for registered_agent_type in all_agent_types {
            if registered_agent_type.agent_type.type_name == agent_type {
                return Ok(Some(registered_agent_type));
            }
        }
        Ok(None)
    }

    /// Notify the service that a deployment has been made for a given project,
    /// so it can invalidate any related caches
    async fn project_changed(&self, _project_id: &ProjectId) {
        // Default implementation does nothing
    }
}

#[derive(Debug)]
pub struct AgentTypesServiceDefault {
    component_service: Arc<dyn ComponentService>,
    cache: DashMap<ProjectId, BTreeMap<String, RegisteredAgentType>>,
}

impl AgentTypesServiceDefault {
    pub fn new(component_service: Arc<dyn ComponentService>) -> Self {
        Self {
            component_service,
            cache: DashMap::new(),
        }
    }
}

#[async_trait]
impl AgentTypesService for AgentTypesServiceDefault {
    async fn get_all_agent_types(
        &self,
        owner: &ComponentOwner,
    ) -> Result<Vec<RegisteredAgentType>, ComponentError> {
        if let Some(cached) = self.cache.get(&owner.project_id) {
            Ok(cached.values().cloned().collect())
        } else {
            let components = self.component_service.find_by_name(None, owner).await?;
            let mut agent_types = Vec::new();
            for component in components {
                agent_types.extend(component.metadata.agent_types().iter().cloned().map(
                    |agent_type| RegisteredAgentType {
                        agent_type,
                        implemented_by: component.versioned_component_id.component_id.clone(),
                    },
                ));
            }
            agent_types.sort_by_key(|r| r.agent_type.type_name.clone());
            self.cache.insert(
                owner.project_id.clone(),
                BTreeMap::from_iter(
                    agent_types
                        .iter()
                        .map(|r| (r.agent_type.type_name.clone(), r.clone())),
                ),
            );
            Ok(agent_types)
        }
    }

    async fn get_agent_type(
        &self,
        agent_type: &str,
        owner: &ComponentOwner,
    ) -> Result<Option<RegisteredAgentType>, ComponentError> {
        if let Some(cached) = self.cache.get(&owner.project_id) {
            Ok(cached.get(agent_type).cloned())
        } else {
            let all = self.get_all_agent_types(owner).await?;
            for registered_agent_type in all {
                if registered_agent_type.agent_type.type_name == agent_type {
                    return Ok(Some(registered_agent_type));
                }
            }
            Ok(None)
        }
    }

    async fn project_changed(&self, project_id: &ProjectId) {
        self.cache.remove(project_id);
    }
}
