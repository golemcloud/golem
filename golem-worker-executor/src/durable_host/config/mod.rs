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
use golem_common::base_model::render_config_path;
use golem_wasm::ValueAndType;
use golem_wasm::json::ValueAndTypeJsonExtensions;

/// Render an agent-config value (held in executor state as a legacy
/// `ValueAndType`) into the flat string form expected by `wasi:config/store`.
/// Scalars render as their bare JSON string; structured values render as JSON.
fn render_agent_config_value(value: &ValueAndType) -> Option<String> {
    value.to_json_value().ok().map(|json| match json {
        serde_json::Value::String(value) => value,
        other => other.to_string(),
    })
}

/// `wasi:config/store` implementation
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get(&mut self, key: String) -> anyhow::Result<Result<Option<String>, Error>> {
        let path: Vec<String> = key.split('.').map(ToOwned::to_owned).collect();

        if path.is_empty() {
            return Ok(Ok(None));
        }

        let value = self
            .state
            .agent_config
            .get(&path)
            .and_then(render_agent_config_value);

        Ok(Ok(value))
    }

    async fn get_all(&mut self) -> anyhow::Result<Result<Vec<(String, String)>, Error>> {
        let entries = self
            .state
            .agent_config
            .iter()
            .filter_map(|(path, value)| {
                render_agent_config_value(value)
                    .map(|rendered| (render_config_path(path), rendered))
            })
            .collect();

        Ok(Ok(entries))
    }
}
