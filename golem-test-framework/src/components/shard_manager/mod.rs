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

pub mod provided;
pub mod spawned;
pub mod unavailable;

use super::rdb::Rdb;
use super::registry_service::RegistryService;
use crate::components::EnvVarBuilder;
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::RoutingTable;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{Level, warn};

#[async_trait]
pub trait ShardManager: Send + Sync {
    fn grpc_host(&self) -> String;
    fn grpc_port(&self) -> u16;

    async fn kill(&self);

    async fn restart(&self, number_of_shards_override: Option<usize>);

    async fn get_routing_table(&self) -> crate::Result<RoutingTable> {
        // Retry with backoff to tolerate transient port-forward reconnects.
        // The port-forward watchdog restarts in ~500ms, so 10 attempts with a 1s delay give at
        // least ~10s of tolerance before giving up. Each attempt can take longer, because the
        // client retries inside it.
        let max_attempts = 10;
        let retry_delay = Duration::from_secs(1);
        let mut last_err = anyhow!("get_routing_table: no attempts made");

        for attempt in 1..=max_attempts {
            match try_get_routing_table(&self.grpc_host(), self.grpc_port()).await {
                Ok(rt) => return Ok(rt),
                Err(err) => {
                    warn!(
                        attempt,
                        max_attempts,
                        error = %err,
                        "Failed to fetch routing table, retrying..."
                    );
                    last_err = err;
                    tokio::time::sleep(retry_delay).await;
                }
            }
        }

        Err(last_err)
    }
}

/// Through Golem's own client, which follows a standby to the leader: a test environment may run
/// several shard manager replicas behind one address.
async fn try_get_routing_table(host: &str, grpc_port: u16) -> crate::Result<RoutingTable> {
    use golem_service_base::clients::shard_manager::{
        GrpcShardManager, GrpcShardManagerConfig, ShardManager as _,
    };
    GrpcShardManager::new(&GrpcShardManagerConfig {
        host: host.to_string(),
        port: grpc_port,
        ..GrpcShardManagerConfig::default()
    })
    .get_routing_table()
    .await
    .map_err(|err| anyhow!("Unable to fetch the routing table from the shard manager: {err}"))
}

async fn env_vars(
    number_of_shards_override: Option<usize>,
    state_write_timeout_override: Option<Duration>,
    http_port: u16,
    grpc_port: u16,
    rdb: &Arc<dyn Rdb>,
    rdb_private_connection: bool,
    registry_service: &Arc<dyn RegistryService>,
    verbosity: Level,
    otlp: bool,
) -> HashMap<String, String> {
    let mut builder = EnvVarBuilder::golem_service(verbosity)
        .with("GOLEM__GRPC__PORT", grpc_port.to_string())
        .with("GOLEM__HTTP_PORT", http_port.to_string())
        .with_str(
            "GOLEM__REGISTRY_SERVICE__HOST",
            &registry_service.grpc_host(),
        )
        .with(
            "GOLEM__REGISTRY_SERVICE__PORT",
            registry_service.grpc_port().to_string(),
        )
        .with_all(
            // The shard manager's SQL settings live under `persistence`, not `db`: the same key
            // also selects etcd in distributed mode. The sub-keys are identical to DbConfig's.
            rdb.info()
                .env("golem_shard_manager", rdb_private_connection)
                .into_iter()
                .map(|(key, value)| {
                    let key = key
                        .strip_prefix("GOLEM__DB__")
                        .map(|rest| format!("GOLEM__PERSISTENCE__{rest}"))
                        .unwrap_or(key);
                    (key, value)
                })
                .collect(),
        )
        .with_optional_otlp("shard_manager", otlp);

    if let Some(number_of_shards) = number_of_shards_override {
        builder = builder.with("GOLEM__NUMBER_OF_SHARDS", number_of_shards.to_string());
    }
    if let Some(state_write_timeout) = state_write_timeout_override {
        builder = builder.with(
            "GOLEM__STATE_WRITE_TIMEOUT",
            format!("{}ms", state_write_timeout.as_millis()),
        );
    }

    builder.build()
}
