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

use golem_common::SafeDisplay;
use golem_common::model::base64::Base64;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct RdbmsConfig {
    pub pool: RdbmsPoolConfig,
    pub query: RdbmsQueryConfig,
}

impl SafeDisplay for RdbmsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "pool:");
        let _ = writeln!(&mut result, "{}", self.pool.to_safe_string_indented());
        let _ = writeln!(&mut result, "query:");
        let _ = writeln!(&mut result, "{}", self.query.to_safe_string_indented());
        result
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RdbmsQueryConfig {
    pub query_batch: usize,
}

impl SafeDisplay for RdbmsQueryConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "batch size: {}", self.query_batch);
        result
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RdbmsPoolConfig {
    pub max_connections: u32,
    #[serde(with = "humantime_serde")]
    pub eviction_ttl: Duration,
    #[serde(with = "humantime_serde")]
    pub eviction_period: Duration,
    #[serde(with = "humantime_serde")]
    pub acquire_timeout: Duration,
}

impl SafeDisplay for RdbmsPoolConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "max connections: {}", self.max_connections);
        let _ = writeln!(&mut result, "eviction ttl: {:?}", self.eviction_ttl);
        let _ = writeln!(&mut result, "eviction period: {:?}", self.eviction_period);
        let _ = writeln!(&mut result, "acquire timeout: {:?}", self.acquire_timeout);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentCacheConfig {
    pub max_capacity: usize,
    pub max_metadata_capacity: usize,
    pub max_resolved_component_capacity: usize,
    /// Maximum number of local component compile fallback attempts allowed to
    /// run concurrently on this executor. This bounds the transient working set
    /// used after both the in-process cache and compiled artifact store miss.
    /// `0` means unlimited.
    pub max_concurrent_compilations: usize,
    #[serde(with = "humantime_serde")]
    pub time_to_idle: Duration,
}

impl SafeDisplay for ComponentCacheConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "max capacity: {}", self.max_capacity);
        let _ = writeln!(
            &mut result,
            "max metadata capacity: {}",
            self.max_metadata_capacity
        );
        let _ = writeln!(
            &mut result,
            "max resolved component capacity: {}",
            self.max_resolved_component_capacity
        );
        let _ = writeln!(
            &mut result,
            "max concurrent compilations: {}",
            self.max_concurrent_compilations
        );
        let _ = writeln!(&mut result, "time to idle: {:?}", self.time_to_idle);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum AgentTypesServiceConfig {
    Grpc(AgentTypesServiceGrpcConfig),
    Local(AgentTypesServiceLocalConfig),
}

impl SafeDisplay for AgentTypesServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            AgentTypesServiceConfig::Grpc(grpc) => {
                let _ = writeln!(&mut result, "grpc:");
                let _ = writeln!(&mut result, "{}", grpc.to_safe_string_indented());
            }
            AgentTypesServiceConfig::Local(_) => {
                let _ = writeln!(&mut result, "local");
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentTypesServiceGrpcConfig {
    #[serde(with = "humantime_serde")]
    pub cache_time_to_idle: Duration,
}

impl SafeDisplay for AgentTypesServiceGrpcConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "cache time to idle: {:?}",
            self.cache_time_to_idle
        );
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentTypesServiceLocalConfig {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnvironmentStateServiceConfig {
    pub cache_capacity: usize,
    pub cache_ttl: Duration,
    #[serde(with = "humantime_serde")]
    pub cache_eviction_interval: Duration,
}

impl Default for EnvironmentStateServiceConfig {
    fn default() -> Self {
        Self {
            cache_capacity: 1000,
            cache_ttl: Duration::from_mins(5),
            cache_eviction_interval: Duration::from_mins(1),
        }
    }
}

impl SafeDisplay for EnvironmentStateServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "cache_capacity: {}", self.cache_capacity);
        let _ = writeln!(&mut result, "cache_ttl: {:?}", self.cache_ttl);
        let _ = writeln!(
            &mut result,
            "cache_eviction_interval: {:?}",
            self.cache_eviction_interval
        );
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DirectInvocationAuthCacheConfig {
    pub cache_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub cache_ttl: Duration,
    #[serde(with = "humantime_serde")]
    pub cache_eviction_interval: Duration,
}

impl Default for DirectInvocationAuthCacheConfig {
    fn default() -> Self {
        Self {
            cache_capacity: 1024,
            cache_ttl: Duration::from_mins(5),
            cache_eviction_interval: Duration::from_mins(1),
        }
    }
}

impl SafeDisplay for DirectInvocationAuthCacheConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "cache_capacity: {}", self.cache_capacity);
        let _ = writeln!(&mut result, "cache_ttl: {:?}", self.cache_ttl);
        let _ = writeln!(
            &mut result,
            "cache_eviction_interval: {:?}",
            self.cache_eviction_interval
        );
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentWebhooksServiceConfig {
    pub use_https_for_webhook_url: bool,
    pub hmac_key: Base64,
}

impl Default for AgentWebhooksServiceConfig {
    fn default() -> Self {
        Self {
            use_https_for_webhook_url: true,
            hmac_key: Base64(vec![
                0x2b, 0x7e, 0x02, 0xa3, 0x8a, 0x51, 0x30, 0x39, 0x7b, 0x74, 0x1d, 0xdc, 0x60, 0x1f,
                0xb5, 0xfc, 0xdd, 0x09, 0xde, 0xd3, 0x33, 0x25, 0x62, 0x38, 0x17, 0x23, 0xcd, 0x3a,
                0xc9, 0x86, 0x1e, 0x41,
            ]),
        }
    }
}

impl SafeDisplay for AgentWebhooksServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "use_https_for_webhook_url: {:?}",
            self.use_https_for_webhook_url
        );
        let _ = writeln!(&mut result, "hmac_key: *******");
        result
    }
}

impl Default for RdbmsQueryConfig {
    fn default() -> Self {
        Self { query_batch: 50 }
    }
}

impl Default for RdbmsPoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 20,
            eviction_ttl: Duration::from_secs(10 * 60),
            eviction_period: Duration::from_secs(2 * 60),
            acquire_timeout: Duration::from_secs(3),
        }
    }
}

impl Default for ComponentCacheConfig {
    fn default() -> Self {
        Self {
            max_capacity: 32,
            max_metadata_capacity: 16384,
            max_resolved_component_capacity: 1024,
            max_concurrent_compilations: 16,
            time_to_idle: Duration::from_secs(12 * 60 * 60),
        }
    }
}

impl Default for AgentTypesServiceConfig {
    fn default() -> Self {
        Self::Grpc(AgentTypesServiceGrpcConfig::default())
    }
}

impl Default for AgentTypesServiceGrpcConfig {
    fn default() -> Self {
        Self {
            cache_time_to_idle: Duration::from_secs(60),
        }
    }
}
