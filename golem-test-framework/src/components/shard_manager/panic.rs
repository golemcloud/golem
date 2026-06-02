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

use super::ShardManager;
use async_trait::async_trait;
use golem_common::model::RoutingTable;

/// Panic-on-use stub for `ShardManager`. Used in cloud mode when no
/// shard-manager host/port are configured.
///
/// Pass `--shard-manager-grpc-host` and `--shard-manager-grpc-port` to
/// the `cloud` subcommand to enable a real shard-manager connection.
///
/// Operational methods (`grpc_host`, `grpc_port`) panic with a clear message.
/// Lifecycle methods (`kill`, `restart`) are no-ops. `get_routing_table()`
/// returns an error (instead of panicking) so that the throughput benchmark
/// can fall back to the unlabeled single-bucket mode.
pub struct PanicShardManager;

#[async_trait]
impl ShardManager for PanicShardManager {
    fn grpc_host(&self) -> String {
        panic!(
            "shard_manager() requires --shard-manager-grpc-host and \
             --shard-manager-grpc-port to be configured in cloud mode"
        );
    }

    fn grpc_port(&self) -> u16 {
        panic!(
            "shard_manager() requires --shard-manager-grpc-host and \
             --shard-manager-grpc-port to be configured in cloud mode"
        );
    }

    async fn kill(&self) {
        // no-op: cloud mode has no local shard-manager process to kill
    }

    async fn restart(&self, _number_of_shards_override: Option<usize>) {
        // no-op: cloud mode has no local shard-manager process to restart
    }

    /// Returns an `Err` so that callers such as the throughput benchmark can
    /// detect the absence of a shard-manager and fall back gracefully, rather
    /// than panicking.
    async fn get_routing_table(&self) -> crate::Result<RoutingTable> {
        Err(anyhow::anyhow!(
            "shard_manager is not configured in cloud mode; \
             pass --shard-manager-grpc-host and --shard-manager-grpc-port \
             to enable routing table fetch and local/remote RPC labeling"
        ))
    }
}
