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

use prometheus::Registry;

pub mod command_handler;
pub mod launch;
mod router;

#[cfg(test)]
test_r::enable!();

pub struct StartedComponents {
    pub cloud_service: cloud_service::TrafficReadyEndpoints,
    pub component_service: golem_component_service::TrafficReadyEndpoints,
    pub shard_manager: golem_shard_manager::RunDetails,
    pub worker_executor: golem_worker_executor::RunDetails,
    pub worker_service: golem_worker_service::TrafficReadyEndpoints,
    pub prometheus_registry: Registry,
}
