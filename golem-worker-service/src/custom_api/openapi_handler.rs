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
use crate::custom_api::openapi::{HttpApiOpenApiSpec, RichCompiledRouteWithAgentType};
use golem_common::base_model::agent::AgentType;

pub struct OpenApiHandler;

impl OpenApiHandler {
    pub async fn generate_spec<'a>(
        spec_details: &[(&'a AgentType, &'a RichCompiledRoute)],
    ) -> Result<String, String> {
        let routes: Vec<_> = spec_details
            .iter()
            .map(|(agent_type, rich_route)| RichCompiledRouteWithAgentType {
                agent_type,
                details: rich_route,
            })
            .collect();

        let spec = HttpApiOpenApiSpec::from_routes(&routes)?;

        serde_yaml::to_string(&spec.0).map_err(|e| e.to_string())
    }
}
