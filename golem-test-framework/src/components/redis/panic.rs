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

use super::Redis;
use async_trait::async_trait;

/// Panic-on-use stub for `Redis`. Used in cloud mode where no direct Redis
/// access is available. Operational methods panic; `kill` is a no-op so that
/// `BenchmarkTestDependencies::kill_all()` completes safely.
pub struct PanicRedis;

#[async_trait]
impl Redis for PanicRedis {
    fn assert_valid(&self) {
        panic!("redis() is not available in cloud mode");
    }

    fn private_host(&self) -> String {
        panic!("redis() is not available in cloud mode");
    }

    fn private_port(&self) -> u16 {
        panic!("redis() is not available in cloud mode");
    }

    fn prefix(&self) -> &str {
        panic!("redis() is not available in cloud mode");
    }

    async fn kill(&self) {
        // no-op: cloud mode has no local Redis process to kill
    }
}
