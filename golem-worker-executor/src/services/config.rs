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

#[cfg(test)]
mod tests {
    use test_r::test;

    use golem_worker_executor_base::services::golem_config::{
        make_config_loader, ShardManagerServiceConfig,
    };

    #[test]
    pub fn config_is_loadable() {
        let golem_config = make_config_loader().load().expect("Failed to load config");

        let shard_manager_grpc_port = match &golem_config.shard_manager_service {
            ShardManagerServiceConfig::Grpc(config) => config.port,
            _ => panic!("Expected shard manager service to be grpc"),
        };
        assert_eq!(shard_manager_grpc_port, 9002);
    }
}
