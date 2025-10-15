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
use crate::model::Component;
use crate::service::component::ComponentService;
use async_trait::async_trait;
use golem_common::model::agent::RegisteredAgentType;
use golem_common::model::component::ComponentOwner;
use golem_common::model::ComponentId;
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
}

// NOTE: we cannot cache on this level currently because changes to the set of components
// can happen on other omponent-service nodes too. This should be revisited after the
// atomic deployment changes.
#[derive(Debug)]
pub struct AgentTypesServiceDefault {
    component_service: Arc<dyn ComponentService>,
}

impl AgentTypesServiceDefault {
    pub fn new(component_service: Arc<dyn ComponentService>) -> Self {
        Self { component_service }
    }
}

#[async_trait]
impl AgentTypesService for AgentTypesServiceDefault {
    // NOTE: unlike component service naming, here "get all" returns only the latest version,
    //       and we do the grouping in the service layer, as this is only added as a quickfix.
    //       With atomic deployments the semantics and APIs will change soon anyway,
    //       hence we do not do a bigger cleanup around naming or changes around the repo layer.
    async fn get_all_agent_types(
        &self,
        owner: &ComponentOwner,
    ) -> Result<Vec<RegisteredAgentType>, ComponentError> {
        let components = self.component_service.find_by_name(None, owner).await?;

        let mut latest_components = BTreeMap::<ComponentId, Component>::new();
        for component in components {
            match latest_components.get_mut(&component.versioned_component_id.component_id) {
                Some(existing) => {
                    if existing.versioned_component_id.version
                        < component.versioned_component_id.version
                    {
                        *existing = component;
                    }
                }
                None => {
                    latest_components.insert(
                        component.versioned_component_id.component_id.clone(),
                        component,
                    );
                    continue;
                }
            }
        }

        let mut agent_types = Vec::new();
        for component in latest_components.values() {
            agent_types.extend(component.metadata.agent_types().iter().cloned().map(
                |agent_type| RegisteredAgentType {
                    agent_type,
                    implemented_by: component.versioned_component_id.component_id.clone(),
                },
            ));
        }
        agent_types.sort_by_key(|r| r.agent_type.type_name.clone());
        Ok(agent_types)
    }
}
