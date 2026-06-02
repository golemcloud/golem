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

use super::RedisMonitor;

/// Panic-on-use stub for `RedisMonitor`. Used in cloud mode where no direct
/// Redis access is available. Operational methods panic; `kill` is a no-op so
/// that `BenchmarkTestDependencies::kill_all()` completes safely.
pub struct PanicRedisMonitor;

impl RedisMonitor for PanicRedisMonitor {
    fn assert_valid(&self) {
        panic!("redis_monitor() is not available in cloud mode");
    }

    fn kill(&self) {
        // no-op: cloud mode has no local Redis process to kill
    }
}
