// Copyright 2024-2025 Golem Cloud
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

use prometheus::Registry;

pub mod command;
pub mod launch;
mod migration;
mod router;

#[cfg(test)]
test_r::enable!();

pub struct StartedComponents {
    pub component_service: golem_component_service::TrafficReadyEndpoints,
    pub shard_manager: golem_shard_manager::RunDetails,
    pub worker_executor: golem_worker_executor_base::RunDetails,
    pub worker_service: golem_worker_service::TrafficReadyEndpoints,
    pub prometheus_registy: Registry,
}
