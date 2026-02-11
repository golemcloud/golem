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

use crate::custom_api::RichCompiledRoute;
use crate::custom_api::openapi::{HttpApiDefinitionOpenApiSpec, RouteWithAgentType};
use golem_common::base_model::agent::AgentType;
use golem_service_base::custom_api::{CompiledRoute, RoutesWithAgentType};
use std::collections::HashMap;

pub struct OpenApiHandler;

impl OpenApiHandler {
    pub async fn generate_spec(
        spec_details: Vec<(AgentType, RichCompiledRoute)>,
        security_scheme_details: &HashMap<String, String>,
    ) -> Result<String, String> {
        let routes = spec_details
            .iter()
            .map(|(agent_type, rich_route)| RouteWithAgentType {
                agent_type: agent_type.clone(),
                details: rich_route.clone(),
            })
            .collect::<Vec<_>>()
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

        let spec =
            HttpApiDefinitionOpenApiSpec::from_routes(routes, &security_scheme_details).await?;

        serde_yaml::to_string(&spec.0).map_err(|e| e.to_string())
    }
}
