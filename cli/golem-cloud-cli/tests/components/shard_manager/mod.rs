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
use std::time::Duration;

use async_trait::async_trait;
use tracing::Level;

use crate::components::redis::Redis;
use crate::components::wait_for_startup_grpc;

pub mod spawned;

#[async_trait]
pub trait ShardManager {
    fn private_host(&self) -> String;
    fn private_http_port(&self) -> u16;
    fn private_grpc_port(&self) -> u16;

    fn public_host(&self) -> String {
        self.private_host()
    }

    fn public_http_port(&self) -> u16 {
        self.private_http_port()
    }

    fn public_grpc_port(&self) -> u16 {
        self.private_grpc_port()
    }

    fn kill(&self);
    async fn restart(&self);

    fn blocking_restart(&self) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move { self.restart().await });
    }
}

async fn wait_for_startup(host: &str, grpc_port: u16, timeout: Duration) {
    wait_for_startup_grpc(host, grpc_port, "cloud-shard-manager", timeout).await
}

fn env_vars(
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

    HashMap::from_iter(env.iter().map(|(k, v)| (k.to_string(), v.to_string())))
}
