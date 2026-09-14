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

mod campaign_shutdown;
mod etcd_backed;
mod shard_management;

use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
use golem_test_framework::components::etcd::docker_etcd::DockerEtcd;
use std::sync::Arc;
use test_r::{sequential_suite, test_dep};

test_r::enable!();

sequential_suite!(etcd_backed);

#[derive(Debug)]
pub struct Tracing;

#[test_dep(scope = PerWorker)]
pub fn tracing() -> Tracing {
    init_tracing_with_default_debug_env_filter(
        &TracingConfig::test_pretty_without_time("shard-manager-tests").with_env_overrides(),
    );
    Tracing
}

/// One etcd server per test worker, shared by every module under `etcd_backed`.
///
/// Declared here rather than per module so they do not each start a container; tests isolate
/// themselves within it by state key and election name.
#[test_dep(scope = PerWorker)]
pub async fn etcd(_tracing: &Tracing) -> Arc<DockerEtcd> {
    Arc::new(DockerEtcd::new().await)
}
