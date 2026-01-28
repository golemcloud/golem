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
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::custom_api::CompiledRoutes;
use std::sync::Arc;

#[async_trait]
pub trait HttpApiDefinitionsLookup: Send + Sync {
    async fn get(&self, domain: &Domain) -> Result<CompiledRoutes, ApiDefinitionLookupError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ApiDefinitionLookupError {
    #[error("No routes found for site {0}")]
    UnknownSite(Domain),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(ApiDefinitionLookupError, RegistryServiceError);

impl SafeDisplay for ApiDefinitionLookupError {
    fn to_safe_string(&self) -> String {
        match self {
            ApiDefinitionLookupError::InternalError(_) => "Internal error".to_string(),
            ApiDefinitionLookupError::UnknownSite(_) => "Unknown authority".to_string(),
        }
    }
}

// Note: No caching here, the final routers are cached in the RouteResolver
pub struct RegistryServiceApiDefinitionsLookup {
    registry_service_client: Arc<dyn RegistryService>,
}

impl RegistryServiceApiDefinitionsLookup {
    pub fn new(registry_service_client: Arc<dyn RegistryService>) -> Self {
        Self {
            registry_service_client,
        }
    }
}

#[async_trait]
impl HttpApiDefinitionsLookup for RegistryServiceApiDefinitionsLookup {
    async fn get(&self, domain: &Domain) -> Result<CompiledRoutes, ApiDefinitionLookupError> {
        self.registry_service_client
            .get_active_routes_for_domain(domain)
            .await
            .map_err(|e| match e {
                RegistryServiceError::NotFound(_) => {
                    ApiDefinitionLookupError::UnknownSite(domain.clone())
                }
                other => other.into(),
            })
    }
}
