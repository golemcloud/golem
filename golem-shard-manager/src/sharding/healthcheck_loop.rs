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

use super::healthcheck::HealthCheck;
use super::shard_management::ShardManagement;
use crate::config::HealthCheckConfig;
use crate::sharding::healthcheck::get_unhealthy_executors;
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::{Instrument, debug, warn};

pub fn start_health_check_loop(
    shard_management: Arc<ShardManagement>,
    health_check: Arc<dyn HealthCheck>,
    config: &HealthCheckConfig,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) {
    let delay = config.delay;

    join_set.spawn(
        async move {
            loop {
                tokio::time::sleep(delay).await;
                run_health_check(&shard_management, &health_check).await
            }
        }
        .in_current_span(),
    );
}

async fn run_health_check(
    shard_management: &Arc<ShardManagement>,
    health_check: &Arc<dyn HealthCheck>,
) {
    debug!("Scheduled to conduct health check");
    let shard_state = shard_management.current_snapshot().await;
    debug!("Checking health of registered executors...");
    let executors = shard_state.get_executors_with_addrs();
    let failed_executors = get_unhealthy_executors(health_check, &executors).await;
    if failed_executors.is_empty() {
        debug!("All registered executors are healthy")
    } else {
        for (executor_id, addr, pod_name) in executors
            .into_iter()
            .filter(|(id, _, _)| failed_executors.contains(id))
        {
            warn!(
                executor_id = %executor_id,
                addr = %addr,
                pod_name = pod_name.as_deref().unwrap_or(""),
                "Executor was found to be unhealthy; unregistering"
            );
            shard_management.unregister_executor(executor_id).await;
        }
    }

    debug!("Finished checking health of registered executors");
}
