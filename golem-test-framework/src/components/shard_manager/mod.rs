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
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tracing::Level;

use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_client::ShardManagerServiceClient;

use crate::components::redis::Redis;
use crate::components::{wait_for_startup_grpc, EnvVarBuilder, GolemEnvVars};

pub mod docker;
pub mod k8s;
pub mod provided;
pub mod spawned;

#[async_trait]
pub trait ShardManager {
    async fn client(&self) -> ShardManagerServiceClient<Channel> {
        new_client(&self.public_host(), self.public_grpc_port()).await
    }

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

    async fn kill(&self);
    async fn restart(&self, number_of_shards_override: Option<usize>);
}

async fn new_client(host: &str, grpc_port: u16) -> ShardManagerServiceClient<Channel> {
    ShardManagerServiceClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to golem-shard-manager")
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip)
}

async fn wait_for_startup(host: &str, grpc_port: u16, timeout: Duration) {
    wait_for_startup_grpc(host, grpc_port, "golem-shard-manager", timeout).await
}

#[async_trait]
pub trait ShardManagerEnvVars {
    async fn env_vars(
        &self,
        number_of_shards_override: Option<usize>,
        http_port: u16,
        grpc_port: u16,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        verbosity: Level,
    ) -> HashMap<String, String>;
}

#[async_trait]
impl ShardManagerEnvVars for GolemEnvVars {
    async fn env_vars(
        &self,
        number_of_shards_override: Option<usize>,
        http_port: u16,
        grpc_port: u16,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        verbosity: Level,
    ) -> HashMap<String, String> {
        let mut builder = EnvVarBuilder::golem_service(verbosity)
            .with("GOLEM_SHARD_MANAGER_PORT", grpc_port.to_string())
            .with("GOLEM__HTTP_PORT", http_port.to_string())
            .with("GOLEM__REDIS__HOST", redis.private_host())
            .with_str("GOLEM__REDIS__KEY_PREFIX", redis.prefix())
            .with("GOLEM__REDIS__PORT", redis.private_port().to_string())
            .with("REDIS__HOST", redis.private_host());

        if let Some(number_of_shards) = number_of_shards_override {
            builder = builder.with("GOLEM__NUMBER_OF_SHARDS", number_of_shards.to_string());
        }

        builder.build()
    }
}
