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

pub mod command;
mod health;
pub mod launch;
mod migration;
mod proxy;

#[cfg(test)]
test_r::enable!();

pub struct AllRunDetails {
    pub component_service: golem_component_service::RunDetails,
    pub shard_manager: golem_shard_manager::RunDetails,
    pub worker_executor: golem_worker_executor_base::RunDetails,
    pub worker_service: golem_worker_service::RunDetails,
}

impl AllRunDetails {
    pub fn healthcheck_ports(&self) -> Vec<u16> {
        vec![
            self.component_service.http_port,
            self.shard_manager.http_port,
            self.worker_service.http_port,
            self.worker_executor.http_port,
        ]
    }
}
