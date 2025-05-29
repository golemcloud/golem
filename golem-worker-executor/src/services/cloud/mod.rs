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

// Golem Cloud specific services (to be merged with the `oss` module once everything else is merged).

use std::sync::Arc;

use crate::workerctx::cloud::Context;
use crate::services::HasExtraDeps;
use crate::services::cloud::config::AdditionalGolemConfig;

pub mod component;
pub mod config;
pub mod plugins;
pub mod resource_limits;

pub trait HasAdditionalGolemConfig {
    fn additional_golem_config(&self) -> Arc<config::AdditionalGolemConfig>;
}

pub trait HasResourceLimits {
    fn resource_limits(&self) -> Arc<dyn resource_limits::ResourceLimits + Send + Sync>;
}

#[derive(Clone)]
pub struct AdditionalDeps {
    additional_golem_config: Arc<AdditionalGolemConfig>,
    resource_limits: Arc<dyn resource_limits::ResourceLimits + Send + Sync>,
}

impl AdditionalDeps {
    pub fn new(
        additional_golem_config: Arc<AdditionalGolemConfig>,
        resource_limits: Arc<dyn resource_limits::ResourceLimits + Send + Sync>,
    ) -> Self {
        Self {
            additional_golem_config,
            resource_limits,
        }
    }

    #[cfg(test)]
    #[allow(unused)]
    pub async fn mocked() -> Self {
        let resource_limits = Arc::new(resource_limits::ResourceLimitsMock::new());
        Self {
            additional_golem_config: Arc::new(AdditionalGolemConfig::default()),
            resource_limits,
        }
    }
}

impl HasAdditionalGolemConfig for AdditionalDeps {
    fn additional_golem_config(&self) -> Arc<AdditionalGolemConfig> {
        self.additional_golem_config.clone()
    }
}

impl HasResourceLimits for AdditionalDeps {
    fn resource_limits(&self) -> Arc<dyn resource_limits::ResourceLimits + Send + Sync> {
        self.resource_limits.clone()
    }
}

impl<T: HasExtraDeps<Context>> HasAdditionalGolemConfig for T {
    fn additional_golem_config(&self) -> Arc<AdditionalGolemConfig> {
        self.extra_deps().additional_golem_config.clone()
    }
}

impl<T: HasExtraDeps<Context>> HasResourceLimits for T {
    fn resource_limits(&self) -> Arc<dyn resource_limits::ResourceLimits + Send + Sync> {
        self.extra_deps().resource_limits.clone()
    }
}
