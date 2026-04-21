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

use super::DurableWorkerCtx;
use crate::preview2::wasi::config::store::{Error, Host};
use crate::workerctx::WorkerCtx;
use golem_common::model::worker::TypedAgentConfigEntry;

/// `wasi:config/store` implementation
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get(&mut self, key: String) -> anyhow::Result<Result<Option<String>, Error>> {
        let path: Vec<String> = key
            .split('.')
            .map(ToOwned::to_owned)
            .collect();

        if path.is_empty() {
            return Ok(Ok(None));
        }

        let value = self
            .state
            .agent_config
            .get(&path)
            .and_then(|value| {
                TypedAgentConfigEntry {
                    path,
                    value: value.clone(),
                }
                .to_flat_pair()
                .map(|(_, rendered_value)| rendered_value)
            });

        Ok(Ok(value))
    }

    async fn get_all(&mut self) -> anyhow::Result<Result<Vec<(String, String)>, Error>> {
        let entries = self
            .state
            .agent_config
            .iter()
            .map(|(path, value)| TypedAgentConfigEntry {
                path: path.clone(),
                value: value.clone(),
            })
            .collect::<Vec<_>>();

        Ok(Ok(TypedAgentConfigEntry::to_flat_map(&entries)
            .into_iter()
            .collect()))
    }
}
