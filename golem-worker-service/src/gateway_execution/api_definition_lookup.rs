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

use async_trait::async_trait;
use golem_common::model::domain_registration::Domain;
use golem_common::SafeDisplay;
use golem_service_base::custom_api::compiled_http_api_definition::CompiledHttpApiDefinition;

#[async_trait]
pub trait HttpApiDefinitionsLookup: Send + Sync {
    async fn get(
        &self,
        domain: &Domain,
    ) -> Result<Vec<CompiledHttpApiDefinition>, ApiDefinitionLookupError>;
}

pub enum ApiDefinitionLookupError {
    UnknownSite(Domain),
    InternalError(anyhow::Error),
}

impl SafeDisplay for ApiDefinitionLookupError {
    fn to_safe_string(&self) -> String {
        match self {
            ApiDefinitionLookupError::InternalError(_) => "Internal error".to_string(),
            ApiDefinitionLookupError::UnknownSite(_) => "Unknown authority".to_string(),
        }
    }
}

pub struct StubHttpApiDefinitionsLookup;

#[async_trait]
impl HttpApiDefinitionsLookup for StubHttpApiDefinitionsLookup {
    async fn get(
        &self,
        _domain: &Domain,
    ) -> Result<Vec<CompiledHttpApiDefinition>, ApiDefinitionLookupError> {
        unimplemented!()
    }
}
