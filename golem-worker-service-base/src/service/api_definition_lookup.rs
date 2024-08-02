// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::fmt::Display;
use std::sync::Arc;

use crate::api_definition::http::CompiledHttpApiDefinition;
use crate::http::InputHttpRequest;
use crate::service::api_deployment::ApiDeploymentService;
use async_trait::async_trait;
use tracing::error;

// TODO; We could optimise this further
// to pick the exact API Definition (instead of a vector),
// by doing route resolution at this stage rather than
// delegating that task to worker-binding resolver.
// However, requires lot more work.
#[async_trait]
pub trait ApiDefinitionsLookup<Input, ApiDefinition> {
    async fn get(&self, input: Input) -> Result<Vec<ApiDefinition>, ApiDefinitionLookupError>;
}

pub struct ApiDefinitionLookupError(pub String);

impl Display for ApiDefinitionLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ApiDefinitionLookupError: {}", self.0)
    }
}

pub struct HttpApiDefinitionLookup<Namespace> {
    deployment_service: Arc<dyn ApiDeploymentService<Namespace> + Sync + Send>,
}

impl<Namespace> HttpApiDefinitionLookup<Namespace> {
    pub fn new(deployment_service: Arc<dyn ApiDeploymentService<Namespace> + Sync + Send>) -> Self {
        Self { deployment_service }
    }
}

#[async_trait]
impl<Namespace> ApiDefinitionsLookup<InputHttpRequest, CompiledHttpApiDefinition>
    for HttpApiDefinitionLookup<Namespace>
{
    async fn get(
        &self,
        input_http_request: InputHttpRequest,
    ) -> Result<Vec<CompiledHttpApiDefinition>, ApiDefinitionLookupError> {
        // HOST should exist in Http Request
        let host = input_http_request
            .get_host()
            .ok_or(ApiDefinitionLookupError(
                "Host header not found".to_string(),
            ))?;

        let http_api_defs = self
            .deployment_service
            .get_definitions_by_site(&host)
            .await
            .map_err(|err| {
                error!("Error getting API definitions from the repo: {}", err);
                ApiDefinitionLookupError(format!(
                    "Error getting API definitions from the repo: {}",
                    err
                ))
            })?;

        if http_api_defs.is_empty() {
            return Err(ApiDefinitionLookupError(format!(
                "API deployment with site: {} not found",
                &host
            )));
        }

        Ok(http_api_defs)
    }
}
