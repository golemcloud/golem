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

use std::sync::Arc;
use async_trait::async_trait;
use golem_common::base_model::domain_registration::Domain;
use golem_common::{error_forwarding, SafeDisplay};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::mcp::CompiledMcp;

#[async_trait]
pub trait McpCapabilityLookup {
    async fn get(&self, domain: &Domain) -> Result<CompiledMcp, McpCapabilitiesLookupError>;
}


#[derive(Debug, thiserror::Error)]
pub enum McpCapabilitiesLookupError {
    #[error("No mcp capabilities found for site {0}")]
    UnknownSite(Domain),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(McpCapabilitiesLookupError, RegistryServiceError);

impl SafeDisplay for McpCapabilitiesLookupError {
    fn to_safe_string(&self) -> String {
        match self {
            McpCapabilitiesLookupError::InternalError(_) => "Internal error".to_string(),
            McpCapabilitiesLookupError::UnknownSite(_) => "Unknown authority".to_string(),
        }
    }
}

// Note: No caching here, the caching is part of MCP session
pub struct RegistryServiceMcpCapabilityLookup {
    registry_service_client: Arc<dyn RegistryService>,
}


impl RegistryServiceMcpCapabilityLookup {
    pub fn new(registry_service_client: Arc<dyn RegistryService>) -> Self {
        Self {
            registry_service_client,
        }
    }
}

#[async_trait]
impl McpCapabilityLookup for RegistryServiceMcpCapabilityLookup {
    async fn get(&self, domain: &Domain) -> Result<CompiledMcp, McpCapabilitiesLookupError> {
        self.registry_service_client
            .get_active_mcp_capabilities_for_domain(domain)
            .await
            .map_err(|e| e.into())
     }
}
