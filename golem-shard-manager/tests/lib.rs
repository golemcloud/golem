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
mod shard_management;

use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
use test_r::{sequential_suite, test_dep};

test_r::enable!();

// The etcd dimension shares one server per worker and the fixed `STATE_KEY`, and every store wipes
// that key when it connects, so two persistence tests running at once would see each other's
// writes as revision conflicts.
sequential_suite!(persistence);

#[derive(Debug)]
pub struct Tracing;

#[test_dep(scope = PerWorker)]
pub fn tracing() -> Tracing {
    init_tracing_with_default_debug_env_filter(
        &TracingConfig::test_pretty_without_time("shard-manager-tests").with_env_overrides(),
    );
    Tracing
}
