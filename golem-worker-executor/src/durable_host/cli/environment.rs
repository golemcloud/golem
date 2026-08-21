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

use crate::durable_host::authorization::targets::{agent_owner, env_target};
use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, NotCancellable};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::model::AgentConfig;
use crate::services::HasWorker;
use crate::services::oplog::OplogOps;
use crate::worker::merge_agent_env_with_default_env;
use crate::workerctx::WorkerCtx;
use golem_common::model::AgentId;
use golem_common::model::card::PermissionTarget;
use golem_common::model::oplog::host_functions::WasiCliEnvironmentGetEnvironment;
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostRequestCliEnvironmentGetEnvironment,
    HostResponseCliEnvironmentGetEnvironment, OplogEntry, OplogIndex,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use wasmtime_wasi::cli::WasiCliView as _;
use wasmtime_wasi::p2::bindings::cli::environment::Host;

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    /// Builds the deterministic enriched worker environment: the worker metadata env merged with
    /// the agent type's default env, plus the Golem-provided variables (`GOLEM_AGENT_ID`,
    /// `GOLEM_WORKER_NAME`, `GOLEM_COMPONENT_ID`, ...). Shared by the P2 and P3
    /// `cli::environment` host implementations. This only reconstructs the existing worker
    /// environment; the filtered guest-visible result is recorded separately by the host call.
    fn build_unfiltered_environment(&self) -> wasmtime::Result<Vec<(String, String)>> {
        let default_agent_env = self
            .agent_type_provision_config()
            .map(|c| c.env.clone())
            .unwrap_or_default();

        let worker_metadata = self.public_state.worker().get_initial_worker_metadata();
        let mut env =
            merge_agent_env_with_default_env(Some(worker_metadata.env), default_agent_env);

        let current_agent_name = if let Some(agent_id) = self.parsed_agent_id() {
            let updated_agent_id = agent_id
                .with_phantom_id(self.state.current_phantom_id)
                .map_err(wasmtime::Error::msg)?;
            updated_agent_id.to_string()
        } else {
            self.owned_agent_id.agent_name()
        };

        AgentConfig::enrich_env(
            &mut env,
            &AgentId {
                component_id: self.owned_agent_id.component_id(),
                agent_id: current_agent_name,
            },
            &self.state.agent_id.as_ref().map(|id| id.agent_type.clone()),
            self.state.component_metadata.revision,
        );

        Ok(env)
    }

    fn build_filtered_environment(
        &self,
    ) -> wasmtime::Result<(Vec<(String, String)>, Vec<(PermissionTarget, bool)>)> {
        let owner = agent_owner(self);
        let environment = self
            .build_unfiltered_environment()?
            .into_iter()
            .filter_map(|(name, value)| {
                let target = env_target(owner.clone(), &name).ok()?;
                Some((name, value, target))
            })
            .collect::<Vec<_>>();
        let decisions = environment
            .iter()
            .map(|(_, _, target)| {
                let allowed = self.operator_authorizes_current_invocation()
                    || self
                        .state
                        .agent_effective_surface
                        .authorize(target)
                        .unwrap_or(false);
                (target.clone(), allowed)
            })
            .collect::<Vec<_>>();
        let filtered = environment
            .into_iter()
            .zip(&decisions)
            .filter_map(|((name, value, _), (_, allowed))| allowed.then_some((name, value)))
            .collect();
        Ok((filtered, decisions))
    }

    async fn load_recorded_environment_request(
        &self,
        start_index: OplogIndex,
    ) -> Result<HostRequestCliEnvironmentGetEnvironment, WorkerExecutorError> {
        let entry = self.state.oplog.read(start_index).await;
        let request = match entry {
            OplogEntry::Start {
                function_name,
                request: Some(request),
                ..
            } if function_name == WasiCliEnvironmentGetEnvironment::HOST_FUNCTION_NAME => request,
            other => {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "cli::environment.get-environment Start with a request",
                    format!("{other:?}"),
                ));
            }
        };
        let request: HostRequest =
            self.state
                .oplog
                .download_payload(request)
                .await
                .map_err(|error| {
                    WorkerExecutorError::runtime(format!(
                        "failed to load durable environment request: {error}"
                    ))
                })?;
        request.try_into().map_err(|error| {
            WorkerExecutorError::unexpected_oplog_entry(
                "cli::environment.get-environment request payload",
                error,
            )
        })
    }

    pub(crate) async fn get_durable_environment(
        &mut self,
    ) -> wasmtime::Result<Vec<(String, String)>> {
        let (begun, captured) = CallHandle::<
            WasiCliEnvironmentGetEnvironment,
            NotCancellable,
        >::begin_with_agent_authority_capture(
            self,
            DurableFunctionType::ReadLocal,
            |ctx| ctx.build_filtered_environment(),
        )
        .await?;
        let (mut call, live_environment) = if begun.is_live() {
            let (environment, decisions) = captured.ok_or_else(|| {
                wasmtime::Error::msg("live environment call has no authority view")
            })??;
            for (target, allowed) in decisions {
                crate::durable_host::record_permission_decisions(
                    std::slice::from_ref(&target),
                    allowed,
                );
            }
            // The request carries the admitted view so an incomplete call can finish after
            // recovery without rebuilding the environment or consulting current authority.
            let call = begun
                .start_live(
                    self,
                    HostRequestCliEnvironmentGetEnvironment {
                        environment: environment.clone(),
                    },
                )
                .await?;
            (call, Some(environment))
        } else {
            (begun.start_replay(self).await?, None)
        };
        if !call.is_live() {
            match call.replay(self).await? {
                CallReplayOutcome::Replayed(response) => return Ok(response.environment),
                CallReplayOutcome::Incomplete(live) => call = live,
            }
        }

        let environment = match live_environment {
            Some(environment) => environment,
            None => {
                self.load_recorded_environment_request(call.start_index())
                    .await?
                    .environment
            }
        };
        Ok(call
            .complete(
                self,
                HostResponseCliEnvironmentGetEnvironment { environment },
            )
            .await?
            .environment)
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_environment(&mut self) -> wasmtime::Result<Vec<(String, String)>> {
        self.get_durable_environment().await
    }

    async fn get_arguments(&mut self) -> wasmtime::Result<Vec<String>> {
        // NOTE: No need to persist the results of this function as the result values are persisted as part of the initial Create oplog entry
        self.observe_function_call("cli::environment", "get_arguments");
        let mut view = self.as_wasi_view();
        Host::get_arguments(&mut view.cli()).await
    }

    async fn initial_cwd(&mut self) -> wasmtime::Result<Option<String>> {
        // NOTE: No need to persist the results of this function as the result values are persisted as part of the initial Create oplog entry
        self.observe_function_call("cli::environment", "initial_cwd");
        let mut view = self.as_wasi_view();
        Host::initial_cwd(&mut view.cli()).await
    }
}
