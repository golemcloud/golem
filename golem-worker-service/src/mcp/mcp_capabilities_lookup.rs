// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use golem_common::base_model::domain_registration::Domain;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::mcp::CompiledMcp;
use std::sync::Arc;
use std::time::Duration;

#[async_trait]
pub trait McpCapabilityLookup: Send + Sync {
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

/// TTL-based in-memory cache for compiled MCP lookups, consistent with
/// how HTTP uses `RouteResolver`'s `domain_api_cache`.
const MCP_CACHE_MAX_CAPACITY: usize = 1024;
const MCP_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const MCP_CACHE_EVICTION_PERIOD: Duration = Duration::from_secs(60);

pub struct RegistryServiceMcpCapabilityLookup {
    registry_service_client: Arc<dyn RegistryService>,
    cache: Cache<Domain, (), CompiledMcp, ()>,
}

impl RegistryServiceMcpCapabilityLookup {
    pub fn new(registry_service_client: Arc<dyn RegistryService>) -> Self {
        Self {
            registry_service_client,
            cache: Cache::new(
                Some(MCP_CACHE_MAX_CAPACITY),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::OlderThan {
                    ttl: MCP_CACHE_TTL,
                    period: MCP_CACHE_EVICTION_PERIOD,
                },
                "mcp_capability_lookup",
            ),
        }
    }
}

#[async_trait]
impl McpCapabilityLookup for RegistryServiceMcpCapabilityLookup {
    async fn get(&self, domain: &Domain) -> Result<CompiledMcp, McpCapabilitiesLookupError> {
        let registry_client = self.registry_service_client.clone();
        let domain_clone = domain.clone();
        self.cache
            .get_or_insert_simple(domain, async move || {
                registry_client
                    .get_active_compiled_mcps_for_domain(&domain_clone)
                    .await
                    .map_err(|_| ())
            })
            .await
            .map_err(|_| {
                McpCapabilitiesLookupError::InternalError(anyhow::anyhow!(
                    "Failed to get compiled MCP for domain {}",
                    domain.0
                ))
            })
    }
}
