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

use golem_common::model::agent::AgentTypeName;
use golem_service_base::custom_api::{CompiledRoute, RouteBehaviour};
use std::collections::HashMap;

pub struct OpenApiHandler;

impl OpenApiHandler {
    pub async fn generate_spec(
        agent_type_name: &AgentTypeName,
        routes: &[CompiledRoute],
    ) -> Result<String, String> {
        // Filter routes for this agent type
        let agent_routes: Vec<_> = routes
            .iter()
            .filter(|route| {
                if let RouteBehaviour::CallAgent(call_agent) = &route.behavior {
                    call_agent.agent_type == *agent_type_name
                } else {
                    false
                }
            })
            .collect();

        if agent_routes.is_empty() {
            return Err(format!(
                "No HTTP endpoints found for agent type: {}",
                agent_type_name.0
            ));
        }

        // Use the better openapiv3 based implementation from golem-service-base
        let name = golem_common::model::http_api_definition::HttpApiDefinitionName(
            agent_type_name.0.clone(),
        );
        let version = golem_common::model::http_api_definition::HttpApiDefinitionVersion(
            "1.0.0".to_string(),
        );
        let security_schemes = HashMap::new();

        let spec = golem_service_base::custom_api::openapi::HttpApiDefinitionOpenApiSpec::from_routes(
            &name,
            &version,
            agent_routes.iter().map(|r| *r),
            &HashMap::new(),
        )
        .await?;

        serde_yaml::to_string(&spec.0).map_err(|e| e.to_string())
    }
}
