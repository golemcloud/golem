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

use crate::components::redis_monitor::RedisMonitor;

/// A `RedisMonitor` implementation that doesn't actually monitor anything.
///
/// Used by `Hosted` test-dependency reconstruction on worker subprocesses:
/// the parent owns the real `SpawnedRedisMonitor` lifetime;
/// workers only need a value that satisfies the `RedisMonitor` API so
/// existing call sites like `deps.redis_monitor.assert_valid()` keep
/// compiling. `assert_valid` and `kill` are no-ops here because the parent
/// already enforces validity for the singleton Redis instance.
#[derive(Default)]
pub struct ProvidedRedisMonitor;

impl ProvidedRedisMonitor {
    pub fn new() -> Self {
        Self
    }
}

impl RedisMonitor for ProvidedRedisMonitor {
    fn assert_valid(&self) {
        // No-op: the parent's SpawnedRedisMonitor is the source of truth.
    }

    fn kill(&self) {
        // No-op: the parent owns the Redis subprocess lifetime.
    }
}
