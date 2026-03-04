// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::DurableWorkerCtx;
use crate::preview2::wasi::config::store::{Error, Host};
use crate::workerctx::WorkerCtx;

/// `wasi:config/store` implementation
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get(&mut self, key: String) -> anyhow::Result<Result<Option<String>, Error>> {
        Ok(Ok(self
            .state
            .config_vars
            .read()
            .unwrap()
            .get(&key)
            .cloned()))
    }

    async fn get_all(&mut self) -> anyhow::Result<Result<Vec<(String, String)>, Error>> {
        Ok(Ok(self
            .state
            .config_vars
            .read()
            .unwrap()
            .clone()
            .into_iter()
            .collect()))
    }
}
