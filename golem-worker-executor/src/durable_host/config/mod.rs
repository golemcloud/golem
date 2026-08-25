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
use crate::durable_host::authorization::targets::{agent_owner, config_segments_target};
use crate::durable_host::concurrent::{CallReplayOutcome, DurableCallSession, NotCancellable};
use crate::preview2::wasi::config::store::{Error, Host};
use crate::workerctx::WorkerCtx;
use golem_common::base_model::render_config_path;
use golem_common::model::oplog::host_functions::{WasiConfigGet, WasiConfigGetAll};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestConfigGet, HostRequestConfigGetAll,
    HostResponseConfigGetAllResponse, HostResponseConfigGetResponse,
};
use golem_common::schema::TypedSchemaValue;
use golem_common::schema::render::json_value::to_json_value;

const CONFIG_PERMISSION_DENIED: &str = "permission denied";

fn config_error(error: String) -> Error {
    Error::Upstream(error)
}

/// Render an agent-config value (held in executor state as a schema-native
/// [`TypedSchemaValue`]) into the flat string form expected by
/// `wasi:config/store`. Scalars render as their bare JSON string; structured
/// values render as JSON.
fn render_agent_config_value(value: &TypedSchemaValue) -> Option<String> {
    to_json_value(value.graph(), value.root_type(), value.value())
        .ok()
        .map(|json| match json {
            serde_json::Value::String(value) => value,
            other => other.to_string(),
        })
}

/// `wasi:config/store` implementation
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get(&mut self, key: String) -> anyhow::Result<Result<Option<String>, Error>> {
        let path: Vec<String> = key.split('.').map(ToOwned::to_owned).collect();
        let denied = if self.state.is_live() {
            match config_segments_target(agent_owner(self), &path) {
                Ok(target) => self.authorize_live_permission(&target).await?.is_err(),
                Err(_) => true,
            }
        } else {
            false
        };
        let begun = DurableCallSession::<WasiConfigGet, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadLocal,
        )
        .await?;
        let mut call = if begun.is_live() {
            let call = begun.start_live(self, HostRequestConfigGet { key }).await?;
            if denied {
                let response = call
                    .complete(
                        self,
                        HostResponseConfigGetResponse {
                            result: Err(CONFIG_PERMISSION_DENIED.into()),
                        },
                    )
                    .await?;
                return Ok(response.result.map_err(config_error));
            }
            call
        } else {
            begun.start_replay(self).await?
        };
        if !call.is_live() {
            match call.replay(self).await? {
                CallReplayOutcome::Replayed(response) => {
                    return Ok(response.result.map_err(config_error));
                }
                CallReplayOutcome::Incomplete(live) => call = live,
            }
        }
        let value = self
            .state
            .agent_config
            .get(&path)
            .and_then(render_agent_config_value);
        Ok(call
            .complete(self, HostResponseConfigGetResponse { result: Ok(value) })
            .await?
            .result
            .map_err(config_error))
    }

    async fn get_all(&mut self) -> anyhow::Result<Result<Vec<(String, String)>, Error>> {
        let admitted_paths = if self.state.is_live() {
            let paths: Vec<_> = self.state.agent_config.keys().cloned().collect();
            let targets = paths
                .iter()
                .map(|path| config_segments_target(agent_owner(self), path))
                .collect::<Result<Vec<_>, _>>();
            let admitted = match targets {
                Ok(targets) => self.filter_live_permissions(&targets).await?,
                Err(_) => vec![false; paths.len()],
            };
            Some(
                paths
                    .into_iter()
                    .zip(admitted)
                    .filter_map(|(path, allowed)| allowed.then_some(path))
                    .collect::<std::collections::HashSet<_>>(),
            )
        } else {
            None
        };
        let begun = DurableCallSession::<WasiConfigGetAll, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadLocal,
        )
        .await?;
        let mut call = if begun.is_live() {
            begun.start_live(self, HostRequestConfigGetAll {}).await?
        } else {
            begun.start_replay(self).await?
        };
        if !call.is_live() {
            match call.replay(self).await? {
                CallReplayOutcome::Replayed(response) => {
                    return Ok(response.result.map_err(config_error));
                }
                CallReplayOutcome::Incomplete(live) => call = live,
            }
        }
        let candidates: Vec<_> = self
            .state
            .agent_config
            .iter()
            .filter(|(path, _)| {
                admitted_paths
                    .as_ref()
                    .is_none_or(|paths| paths.contains(*path))
            })
            .filter_map(|(path, value)| {
                render_agent_config_value(value).map(|rendered| (path.clone(), rendered))
            })
            .collect();
        let mut entries: Vec<_> = candidates
            .into_iter()
            .map(|(path, value)| (render_config_path(&path), value))
            .collect();

        entries.sort_by(|(left_key, _), (right_key, _)| left_key.cmp(right_key));

        Ok(call
            .complete(
                self,
                HostResponseConfigGetAllResponse {
                    result: Ok(entries),
                },
            )
            .await?
            .result
            .map_err(config_error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn permission_denial_uses_the_existing_upstream_error_case() {
        assert!(matches!(
            config_error(CONFIG_PERMISSION_DENIED.to_string()),
            Error::Upstream(message) if message == CONFIG_PERMISSION_DENIED
        ));
    }
}
