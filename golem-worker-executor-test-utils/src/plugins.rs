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
use golem_common::model::component::{
    ComponentId, ComponentRevision, InstalledPlugin, PluginPriority,
};
use golem_common::model::plugin_registration::PluginRegistrationId;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::plugin_registration::PluginRegistration;
use golem_worker_executor::services::plugins::PluginsService;

#[derive(Clone)]
pub struct PluginsUnavailable;

#[async_trait]
impl PluginsService for PluginsUnavailable {
    async fn get_plugin_installation(
        &self,
        _component_id: &ComponentId,
        _component_version: ComponentRevision,
        _plugin_priority: PluginPriority,
    ) -> Result<InstalledPlugin, WorkerExecutorError> {
        Err(WorkerExecutorError::runtime("Not available"))
    }

    async fn get_plugin_definition(
        &self,
        _plugin_id: &PluginRegistrationId,
    ) -> Result<PluginRegistration, WorkerExecutorError> {
        Err(WorkerExecutorError::runtime("Not available"))
    }
}
