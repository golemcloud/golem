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

use std::time::Duration;

use tokio::runtime::Handle;
use tokio::task::JoinSet;

/// Spawns a background task that periodically samples mimalloc allocator
/// memory accounting into the Prometheus gauges defined in
/// [`crate::metrics::mimalloc`].
///
/// `sampling_interval` controls how often the gauges are refreshed from
/// `mi_process_info`; Prometheus scrapes the rendered values independently.
///
/// The gauges live in the default `prometheus` registry, so they are
/// rendered on the executor's existing `/metrics` endpoint without any
/// additional wiring.
pub fn install_runtime_metrics(
    runtime: Handle,
    sampling_interval: Duration,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) {
    join_set.spawn_on(
        async move {
            let mut interval = tokio::time::interval(sampling_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                crate::metrics::mimalloc::sample();
            }
        },
        &runtime,
    );
}
