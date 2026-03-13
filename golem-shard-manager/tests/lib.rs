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

mod persistence;

use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_test_framework::components::redis::spawned::SpawnedRedis;
use golem_test_framework::components::redis::Redis;
use std::net::TcpListener;
use std::sync::Arc;
use test_r::test_dep;
use tracing::Level;

test_r::enable!();

#[derive(Debug)]
pub struct Tracing;

#[test_dep]
pub fn tracing() -> Tracing {
    init_tracing_with_default_debug_env_filter(
        &TracingConfig::test_pretty_without_time("shard-manager-tests").with_env_overrides(),
    );
    Tracing
}

#[derive(Clone)]
pub struct ShardManagerTestDependencies {
    pub redis: Arc<dyn Redis + Send + Sync>,
}

impl std::fmt::Debug for ShardManagerTestDependencies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShardManagerTestDependencies")
    }
}

#[test_dep]
pub async fn test_dependencies(_tracing: &Tracing) -> ShardManagerTestDependencies {
    let port = available_port();
    let redis: Arc<dyn Redis + Send + Sync> = Arc::new(SpawnedRedis::new(
        port,
        "".to_string(),
        Level::INFO,
        Level::ERROR,
    ));
    redis.assert_valid();
    ShardManagerTestDependencies { redis }
}

fn available_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("Failed to bind to an ephemeral port")
        .local_addr()
        .expect("Failed to get local address for ephemeral port")
        .port()
}
