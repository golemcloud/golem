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

use super::{DbInfo, Rdb};
use async_trait::async_trait;

/// An `Rdb` that is not directly reachable (e.g. cloud mode, where the
/// database sits behind the Gateway). Lifecycle teardown (`kill`) is a no-op so
/// that `kill_all()` completes; operational methods panic with a clear message.
pub struct UnavailableRdb;

#[async_trait]
impl Rdb for UnavailableRdb {
    fn info(&self) -> DbInfo {
        panic!("rdb() is not available in cloud mode");
    }

    async fn kill(&self) {}
}
