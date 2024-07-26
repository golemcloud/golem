// Copyright 2024 Golem Cloud
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

use std::collections::HashMap;
use std::sync::Arc;

use crate::components::CloudEnvVars;
use async_trait::async_trait;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::shard_manager::ShardManagerEnvVars;
use tracing::Level;

#[async_trait]
impl ShardManagerEnvVars for CloudEnvVars {
    async fn env_vars(
        &self,
        number_of_shards_override: Option<usize>,
        http_port: u16,
        grpc_port: u16,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        verbosity: Level,
    ) -> HashMap<String, String> {
        let log_level = verbosity.as_str().to_lowercase();

        let env: &[(&str, &str)] = &[
            ("RUST_LOG", &format!("{log_level},h2=warn,cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn")),
            ("RUST_BACKTRACE", "1"),
            ("REDIS__HOST", &redis.private_host()),
            ("GOLEM__REDIS__HOST", &redis.private_host()),
            ("GOLEM__REDIS__PORT", &redis.private_port().to_string()),
            ("GOLEM__REDIS__KEY_PREFIX", redis.prefix()),
            ("GOLEM_SHARD_MANAGER_PORT", &grpc_port.to_string()),
            ("GOLEM__HTTP_PORT", &http_port.to_string()),
            ("GOLEM__HEALTH_CHECK__MODE__TYPE", "Grpc"),
        ];

        let mut env = HashMap::from_iter(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));

        if let Some(number_of_shards) = number_of_shards_override {
            env.insert(
                "GOLEM__NUMBER_OF_SHARDS".to_string(),
                number_of_shards.to_string(),
            );
        };

        env
    }
}
