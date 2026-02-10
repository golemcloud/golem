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
use serde_json::json;

/// Handles OpenAPI spec generation for a specific agent type
pub struct OpenApiHandler;

impl OpenApiHandler {
    /// Generate OpenAPI YAML spec for routes matching an agent type
    pub fn generate_spec(
        agent_type_name: &AgentTypeName,
        routes: &[CompiledRoute],
    ) -> Result<String, String> {
        // Filter routes for this agent type
        let agent_routes: Vec<_> = routes
            .iter()
            .filter_map(|route| {
                if let RouteBehaviour::CallAgent(call_agent) = &route.behavior {
                    if call_agent.agent_type == *agent_type_name {
                        return Some(route);
                    }
                }
                None
            })
            .collect();

        if agent_routes.is_empty() {
            return Err(format!(
                "No HTTP endpoints found for agent type: {}",
                agent_type_name.0
            ));
        }

        // Build OpenAPI spec
        let mut spec = json!({
            "openapi": "3.0.0",
            "info": {
                "title": format!("API Endpoints for {}", agent_type_name.0),
                "version": "1.0.0",
                "description": format!("Auto-generated OpenAPI specification for agent type: {}", agent_type_name.0)
            },
            "paths": {}
        });

        let paths = spec["paths"]
            .as_object_mut()
            .ok_or("Failed to initialize paths")?;

        // Add each route to the spec
        for route in agent_routes {
            let path_str = Self::path_segments_to_string(&route.path);
            let method = route.method.to_string().to_lowercase();

            if let RouteBehaviour::CallAgent(call_agent) = &route.behavior {
                let operation = json!({
                    "summary": format!("Call method: {}", call_agent.method_name),
                    "operationId": format!("{}_{}", call_agent.agent_type.0.to_lowercase(), call_agent.method_name),
                    "tags": [&call_agent.agent_type.0],
                    "responses": {
                        "200": {
                            "description": "Successful response",
                            "content": {
                                "application/json": {}
                            }
                        },
                        "400": {
                            "description": "Bad request"
                        },
                        "500": {
                            "description": "Internal server error"
                        }
                    },
                    "x-golem-agent-type": &call_agent.agent_type.0,
                    "x-golem-method": &call_agent.method_name,
                });

                paths
                    .entry(path_str)
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .ok_or("Failed to get path object")?
                    .insert(method, operation);
            }
        }

        serde_yaml::to_string(&spec).map_err(|e| e.to_string())
    }

    /// Convert path segments to OpenAPI path string
    fn path_segments_to_string(segments: &[golem_service_base::custom_api::PathSegment]) -> String {
        segments
            .iter()
            .map(|seg| match seg {
                golem_service_base::custom_api::PathSegment::Literal { value } => {
                    format!("/{}", value)
                }
                golem_service_base::custom_api::PathSegment::Variable => "/{id}".to_string(),
                golem_service_base::custom_api::PathSegment::CatchAll => "/{rest}".to_string(),
            })
            .collect::<String>()
    }
}
