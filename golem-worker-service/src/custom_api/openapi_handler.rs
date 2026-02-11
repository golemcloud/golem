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

use golem_service_base::custom_api::{OpenApiSpecPerAgent};
use std::collections::HashMap;
use golem_service_base::custom_api::openapi::RouteWithAgentType;

pub struct OpenApiHandler;

impl OpenApiHandler {
    pub async fn generate_spec(
        spec_details: &Vec<OpenApiSpecPerAgent>,
    ) -> Result<String, String> {
        let routes = spec_details.iter().map(|r| {
            r.routes.iter().map(|route| RouteWithAgentType {
                agent_type: r.agent_type.clone(),
                details: route.clone()
            })
        }).collect::<Vec<_>>().into_iter().flatten().collect::<Vec<_>>();

        let spec = golem_service_base::custom_api::openapi::HttpApiDefinitionOpenApiSpec::from_routes(
            routes,
            &HashMap::new(),
        )
            .await?;

        serde_yaml::to_string(&spec.0).map_err(|e| e.to_string())
    }
}
