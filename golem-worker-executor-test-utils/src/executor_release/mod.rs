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

use golem_worker_executor::services::{active_agents::InvocationLoops, shutdown::Shutdown};
use std::sync::Weak;
use std::time::Duration;

/// The graph sentinel can disappear before independently owned shutdown cleanup has finished.
pub(super) async fn wait_for_release(
    shutdown: &Shutdown,
    invocation_loops: &InvocationLoops,
    released: Weak<()>,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(30), async {
        shutdown.join_tracked().await;
        invocation_loops
            .wait_for_exit()
            .await
            .map_err(anyhow::Error::msg)?;
        while released.upgrade().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    })
    .await
    .map_err(|_| anyhow::anyhow!("the executor was not released within 30 seconds"))?
}

#[cfg(test)]
mod tests;
