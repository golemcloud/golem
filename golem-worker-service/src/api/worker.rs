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

use super::common::ApiEndpointError;
use crate::config::WebSocketTransportLimits;
use crate::metrics::{ActiveHttpApiStream, HttpApiStreamKind};
use crate::service::auth::AuthService;
use crate::service::worker::ConnectWorkerStream;
use crate::service::worker::{WorkerService, proxy_worker_connection};
use futures::StreamExt;
use futures::TryStreamExt;
use golem_common::base_model::api;
use golem_common::model::agent::{AgentMetadataDto, RevertWorkerTarget};
use golem_common::model::auth::TokenSecret;
use golem_common::model::caller::ClaimVisibility;
use golem_common::model::card::StoredCard;
use golem_common::model::component::{CanonicalFilePath, ComponentRevision, PluginPriority};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{OplogCursor, OplogIndex, OplogRange, OplogScanAllowance};
use golem_common::model::oplog::{OplogInclude, PublicOplogEntryWithIndex, PublicOplogPageEntry};
use golem_common::model::{AgentFingerprint, AgentId, OwnedAgentId, PromiseId};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::{AuthCtx, GolemSecurityScheme, WrappedGolemSecuritySchema};
use golem_service_base::model::*;
use golem_service_base::poem::SerdeJson;
use poem::Body;
use poem::web::websocket::{BoxWebSocketUpgraded, WebSocket};
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Binary, Json};
use poem_openapi::*;
use std::sync::Arc;
use std::time::Duration;
use tracing::Instrument;

const WORKER_CONNECT_PING_INTERVAL: Duration = Duration::from_secs(30);
const WORKER_CONNECT_PING_TIMEOUT: Duration = Duration::from_secs(15);

type Result<T> = std::result::Result<T, ApiEndpointError>;

/// Management of one agent, named by its environment and its agent ID. Every endpoint takes an
/// optional `expectedFingerprint`: when it is set and the agent is another incarnation, the
/// request fails as not found.
pub struct WorkerApi {
    worker_service: Arc<WorkerService>,
    auth_service: Arc<dyn AuthService>,
    connect_websocket: WebSocketTransportLimits,
}

#[OpenApi(prefix_path = "/v1/runtime/envs", tag = ApiTags::Worker)]
impl WorkerApi {
    pub fn new(
        worker_service: Arc<WorkerService>,
        auth_service: Arc<dyn AuthService>,
        connect_websocket: WebSocketTransportLimits,
    ) -> Self {
        Self {
            worker_service,
            auth_service,
            connect_websocket,
        }
    }

    /// Delete an agent
    ///
    /// Interrupts and deletes an existing agent.
    #[oai(
        path = "/:environment_id/agents/:agent_id",
        method = "delete",
        operation_id = "delete_worker"
    )]
    async fn delete_worker(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        #[oai(name = "expectedFingerprint")] expected_fingerprint: Query<Option<AgentFingerprint>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<DeleteWorkerResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = OwnedAgentId::new(environment_id.0, &agent_id.0);
        let record = recorded_http_api_request!("delete_worker", agent_id = agent_id.to_string());

        let response = async {
            self.worker_service
                .delete(&agent_id, expected_fingerprint.0, auth)
                .await?;
            Ok(Json(DeleteWorkerResponse {}))
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    /// Complete a promise
    ///
    /// Completes a promise with a given custom array of bytes.
    /// The promise must be previously created from within the agent, and its identifier (a combination of an agent ID, the agent's fingerprint and an oplogIdx) must be sent out to an external caller so it can use this endpoint to mark the promise completed.
    /// The data field is sent back to the agent, and it has no predefined meaning.
    #[oai(
        path = "/:environment_id/agents/:agent_id/complete",
        method = "post",
        operation_id = "complete_promise"
    )]
    async fn complete_promise(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        params: Json<CompleteParameters>,
        token: GolemSecurityScheme,
    ) -> Result<Json<bool>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let record =
            recorded_http_api_request!("complete_promise", agent_id = agent_id.0.to_string());

        let response = async {
            let CompleteParameters {
                oplog_idx,
                fingerprint,
                data,
            } = params.0;
            let promise_id = PromiseId {
                agent_id: agent_id.0,
                fingerprint,
                oplog_idx: OplogIndex::from_u64(oplog_idx),
            };
            let completed = self
                .worker_service
                .complete_promise(environment_id.0, promise_id, data, auth)
                .await?;
            Ok(Json(completed))
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    /// Interrupt an agent
    ///
    /// Interrupts the execution of an agent.
    /// The agent's status will be Interrupted unless the recover-immediately parameter was used, in which case it remains as it was.
    /// An interrupted agent can be still used, and it is going to be automatically resumed the first time it is used.
    /// For example in case of a new invocation, the previously interrupted invocation is continued before the new one gets processed.
    #[oai(
        path = "/:environment_id/agents/:agent_id/interrupt",
        method = "post",
        operation_id = "interrupt_worker"
    )]
    async fn interrupt_worker(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        /// if true will simulate an agent recovery. Defaults to false.
        #[oai(name = "recovery-immediately")]
        recover_immediately: Query<Option<bool>>,
        #[oai(name = "expectedFingerprint")] expected_fingerprint: Query<Option<AgentFingerprint>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<InterruptResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = OwnedAgentId::new(environment_id.0, &agent_id.0);
        let record =
            recorded_http_api_request!("interrupt_worker", agent_id = agent_id.to_string());

        let response = async {
            self.worker_service
                .interrupt(
                    &agent_id,
                    recover_immediately.0.unwrap_or(false),
                    expected_fingerprint.0,
                    auth,
                )
                .await?;
            Ok(Json(InterruptResponse {}))
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    /// Get metadata of an agent
    ///
    /// Returns metadata about an existing agent:
    /// - `agentId` is the ID of the agent
    /// - `agentTypeName` is the agent's type
    /// - `env` is the provided map of environment variables passed to the agent
    /// - `componentRevision` is the revision of the component used by the agent
    /// - `retryCount` is the number of retries the agent did in case of a failure
    /// - `status` is the agent's current status, one of the following:
    ///     - `Running` if the agent is currently executing
    ///     - `Idle` if the agent is waiting for an invocation
    ///     - `Suspended` if the agent was running but is now waiting to be resumed by an event (such as end of a sleep, a promise, etc)
    ///     - `Interrupted` if the agent was interrupted by the user
    ///     - `Retrying` if the agent failed, and an automatic retry was scheduled for it
    ///     - `Failed` if the agent failed and there are no more retries scheduled for it
    ///     - `Exited` if the agent explicitly exited using the exit WASI function
    #[oai(
        path = "/:environment_id/agents/:agent_id",
        method = "get",
        operation_id = "get_worker_metadata"
    )]
    async fn get_worker_metadata(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        #[oai(name = "expectedFingerprint")] expected_fingerprint: Query<Option<AgentFingerprint>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<AgentMetadataDto>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = OwnedAgentId::new(environment_id.0, &agent_id.0);
        let record =
            recorded_http_api_request!("get_worker_metadata", agent_id = agent_id.to_string());

        let response = async {
            let mut response = self
                .worker_service
                .get_metadata(&agent_id, expected_fingerprint.0, auth)
                .await?;
            response.redact_host_managed_values_for_external();
            Ok(Json(response))
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    /// Resume an agent
    #[oai(
        path = "/:environment_id/agents/:agent_id/resume",
        method = "post",
        operation_id = "resume_worker"
    )]
    async fn resume_worker(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        #[oai(name = "expectedFingerprint")] expected_fingerprint: Query<Option<AgentFingerprint>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ResumeResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = OwnedAgentId::new(environment_id.0, &agent_id.0);
        let record = recorded_http_api_request!("resume_worker", agent_id = agent_id.to_string());

        let response = async {
            self.worker_service
                .resume(&agent_id, false, expected_fingerprint.0, auth)
                .await?;
            Ok(Json(ResumeResponse {}))
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    /// Update an agent
    #[oai(
        path = "/:environment_id/agents/:agent_id/update",
        method = "post",
        operation_id = "update_worker"
    )]
    async fn update_worker(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        params: Json<UpdateWorkerRequest>,
        #[oai(name = "expectedFingerprint")] expected_fingerprint: Query<Option<AgentFingerprint>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<UpdateWorkerResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = OwnedAgentId::new(environment_id.0, &agent_id.0);
        let record = recorded_http_api_request!("update_worker", agent_id = agent_id.to_string());

        let response = async {
            let update_attempt_index = self
                .worker_service
                .update(
                    &agent_id,
                    params.0.mode,
                    params.0.target_revision,
                    params.0.disable_wakeup,
                    expected_fingerprint.0,
                    // A REST request records its own caller as the requester.
                    None,
                    auth,
                )
                .await?;
            Ok(Json(UpdateWorkerResponse {
                update_attempt_index: update_attempt_index.into(),
            }))
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    /// Get the oplog of an agent
    ///
    /// Each recorded invocation shows its caller: the principal that started the chain, the
    /// principal that made the call, and the tokens presented at the entry point. With
    /// `include=claims`, the users among the callers also show their verified claims.
    ///
    /// Reads the oplog entries from `from` to `to`, both included. Without `from`, the read starts
    /// at the first entry; without `to`, it goes to the end of the oplog. A `to` after the end of
    /// the oplog ends the read at the end of the oplog. The endpoint refuses a `to` before
    /// `from`. With `query`, the page holds only the entries in the range that match the query.
    /// Without `query`, a read with `to` is a range read: the response accounts its scan.
    /// `scanEntries` and `scanMillis` limit the scan of a range read. Without them, the scan has no
    /// limit, and `count` still limits each page. Without `query`, `lastIndex` is the last index of
    /// the oplog. With `query`, it is the last index of the range in the oplog.
    #[oai(
        path = "/:environment_id/agents/:agent_id/oplog",
        method = "get",
        operation_id = "get_oplog"
    )]
    #[allow(clippy::too_many_arguments)]
    async fn get_oplog(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        from: Query<Option<u64>>,
        count: Query<u64>,
        cursor: Query<Option<OplogCursor>>,
        query: Query<Option<String>>,
        include: Query<Option<OplogInclude>>,
        #[oai(name = "expectedFingerprint")] expected_fingerprint: Query<Option<AgentFingerprint>>,
        /// The inclusive last index of the read. Without `query`, the read is a range read.
        to: Query<Option<u64>>,
        /// The remaining entry allowance of a range read. Goes with `scanMillis`.
        #[oai(name = "scanEntries")]
        scan_entries: Query<Option<u64>>,
        /// The remaining time allowance of a range read, in milliseconds.
        #[oai(name = "scanMillis")]
        scan_millis: Query<Option<u64>>,
        token: GolemSecurityScheme,
    ) -> Result<SerdeJson<GetOplogResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = OwnedAgentId::new(environment_id.0, &agent_id.0);
        let record = recorded_http_api_request!("get_oplog", agent_id = agent_id.to_string());

        let response = async {
            let allowance =
                oplog_scan_allowance(query.0.as_deref(), to.0, scan_entries.0, scan_millis.0)?;
            self.get_oplog_internal(
                agent_id,
                from.0,
                to.0,
                count.0,
                cursor.0,
                query.0,
                expected_fingerprint.0,
                allowance,
                ClaimVisibility::from_include(include.0),
                auth,
            )
            .await
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    #[allow(clippy::too_many_arguments)]
    async fn get_oplog_internal(
        &self,
        agent_id: OwnedAgentId,
        from: Option<u64>,
        to: Option<u64>,
        count: u64,
        cursor: Option<OplogCursor>,
        query: Option<String>,
        expected_fingerprint: Option<AgentFingerprint>,
        allowance: Option<OplogScanAllowance>,
        claims: ClaimVisibility,
        auth: AuthCtx,
    ) -> Result<SerdeJson<GetOplogResponse>> {
        let range = OplogRange::from_indices(from, to).map_err(|error| {
            ApiEndpointError::bad_request(
                api::error_code::INVALID_OPLOG_QUERY_PARAMS,
                golem_common::safe(format!("Invalid oplog range: {error}")),
            )
        })?;
        let mut response = match query {
            Some(query) => {
                self.worker_service
                    .search_oplog(
                        &agent_id,
                        range,
                        cursor,
                        count,
                        query,
                        expected_fingerprint,
                        claims,
                        auth,
                    )
                    .await?
            }
            None => {
                self.worker_service
                    .get_oplog(
                        &agent_id,
                        range,
                        cursor,
                        count,
                        expected_fingerprint,
                        allowance,
                        claims,
                        auth,
                    )
                    .await?
            }
        };

        for entry in &mut response.entries {
            if let PublicOplogPageEntry::Included(entry) = entry {
                entry.entry.redact_host_managed_values_for_external();
            }
        }

        Ok(SerdeJson(response))
    }

    /// Get one oplog entry of an agent
    ///
    /// Oplog pages omit entries whose encoded form is larger than a server limit, and describe
    /// them by their type, timestamp, size and component revision. This endpoint returns such an
    /// entry in full. Pass the component revision of the description to save the server from
    /// scanning the oplog for it. With `include=claims`, the users among the callers also show
    /// their verified claims.
    #[oai(
        path = "/:environment_id/agents/:agent_id/oplog/:oplog_index",
        method = "get",
        operation_id = "get_oplog_entry"
    )]
    #[allow(clippy::too_many_arguments)]
    async fn get_oplog_entry(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        oplog_index: Path<u64>,
        component_revision: Query<Option<ComponentRevision>>,
        include: Query<Option<OplogInclude>>,
        #[oai(name = "expectedFingerprint")] expected_fingerprint: Query<Option<AgentFingerprint>>,
        token: GolemSecurityScheme,
    ) -> Result<SerdeJson<PublicOplogEntryWithIndex>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = OwnedAgentId::new(environment_id.0, &agent_id.0);
        let record = recorded_http_api_request!(
            "get_oplog_entry",
            agent_id = agent_id.to_string(),
            oplog_index = oplog_index.0
        );

        let response = async {
            let mut entry = self
                .worker_service
                .get_oplog_entry(
                    &agent_id,
                    OplogIndex::from_u64(oplog_index.0),
                    component_revision.0,
                    expected_fingerprint.0,
                    ClaimVisibility::from_include(include.0),
                    auth,
                )
                .await?;
            entry.entry.redact_host_managed_values_for_external();
            Ok(SerdeJson(entry))
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    /// List files in an agent
    #[oai(
        path = "/:environment_id/agents/:agent_id/files/:file_name",
        method = "get",
        operation_id = "get_files"
    )]
    async fn get_file(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        file_name: Path<String>,
        #[oai(name = "expectedFingerprint")] expected_fingerprint: Query<Option<AgentFingerprint>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<GetFilesResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = OwnedAgentId::new(environment_id.0, &agent_id.0);
        let record = recorded_http_api_request!("get_file", agent_id = agent_id.to_string());

        let response = async {
            let path = make_component_file_path(file_name.0)?;
            let nodes = self
                .worker_service
                .get_file_system_node(&agent_id, path, expected_fingerprint.0, auth)
                .await?;
            Ok(Json(GetFilesResponse {
                nodes: nodes.into_iter().map(|n| n.into()).collect(),
            }))
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    /// Get the wallet (active permission cards) of an agent.
    ///
    /// Activates the agent if it is not already active.
    #[oai(
        path = "/:environment_id/agents/:agent_id/wallet",
        method = "get",
        operation_id = "get_agent_wallet"
    )]
    async fn get_agent_wallet(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        #[oai(name = "expectedFingerprint")] expected_fingerprint: Query<Option<AgentFingerprint>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<StoredCard>>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = OwnedAgentId::new(environment_id.0, &agent_id.0);
        let record =
            recorded_http_api_request!("get_agent_wallet", agent_id = agent_id.to_string());

        let response = self
            .worker_service
            .get_agent_wallet(&agent_id, expected_fingerprint.0, auth)
            .instrument(record.span.clone())
            .await
            .map_err(Into::into);

        record.result(response).map(Json)
    }

    /// Get contents of a file in an agent
    #[oai(
        path = "/:environment_id/agents/:agent_id/file-contents/:file_name",
        method = "get",
        operation_id = "get_file_content"
    )]
    async fn get_file_content(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        file_name: Path<String>,
        #[oai(name = "expectedFingerprint")] expected_fingerprint: Query<Option<AgentFingerprint>>,
        token: GolemSecurityScheme,
    ) -> Result<Binary<Body>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = OwnedAgentId::new(environment_id.0, &agent_id.0);
        let record = recorded_http_api_request!("get_files", agent_id = agent_id.to_string());

        let response = async {
            let path = make_component_file_path(file_name.0)?;
            let bytes = self
                .worker_service
                .get_file_contents(&agent_id, path, expected_fingerprint.0, auth)
                .await?;
            Ok(Binary(Body::from_bytes_stream(
                bytes.map_err(|e| std::io::Error::other(e.to_string())),
            )))
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    /// Activate a plugin
    ///
    /// The plugin must be one of the installed plugins for the agent's current component revision.
    #[oai(
        path = "/:environment_id/agents/:agent_id/activate-plugin",
        method = "post",
        operation_id = "activate_plugin"
    )]
    async fn activate_plugin(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        #[oai(name = "plugin-priority")] plugin_priority: Query<PluginPriority>,
        #[oai(name = "expectedFingerprint")] expected_fingerprint: Query<Option<AgentFingerprint>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ActivatePluginResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = OwnedAgentId::new(environment_id.0, &agent_id.0);
        let record = recorded_http_api_request!(
            "activate_plugin",
            agent_id = agent_id.to_string(),
            plugin_priority = plugin_priority.to_string()
        );

        let response = async {
            self.worker_service
                .activate_plugin(&agent_id, plugin_priority.0, expected_fingerprint.0, auth)
                .await?;
            Ok(Json(ActivatePluginResponse {}))
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    /// Deactivate a plugin
    ///
    /// The plugin must be one of the installed plugins for the agent's current component revision.
    #[oai(
        path = "/:environment_id/agents/:agent_id/deactivate-plugin",
        method = "post",
        operation_id = "deactivate_plugin"
    )]
    async fn deactivate_plugin(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        #[oai(name = "plugin-priority")] plugin_priority: Query<PluginPriority>,
        #[oai(name = "expectedFingerprint")] expected_fingerprint: Query<Option<AgentFingerprint>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<DeactivatePluginResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = OwnedAgentId::new(environment_id.0, &agent_id.0);
        let record = recorded_http_api_request!(
            "deactivate_plugin",
            agent_id = agent_id.to_string(),
            plugin_priority = plugin_priority.to_string()
        );

        let response = async {
            self.worker_service
                .deactivate_plugin(&agent_id, plugin_priority.0, expected_fingerprint.0, auth)
                .await?;
            Ok(Json(DeactivatePluginResponse {}))
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    /// Revert an agent
    ///
    /// Reverts an agent by undoing either the last few invocations or the last few recorded oplog entries.
    #[oai(
        path = "/:environment_id/agents/:agent_id/revert",
        method = "post",
        operation_id = "revert_worker"
    )]
    async fn revert_worker(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        target: Json<RevertWorkerTarget>,
        #[oai(name = "expectedFingerprint")] expected_fingerprint: Query<Option<AgentFingerprint>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<RevertWorkerResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = OwnedAgentId::new(environment_id.0, &agent_id.0);
        let record = recorded_http_api_request!("revert_worker", agent_id = agent_id.to_string());

        let response = async {
            self.worker_service
                .revert_worker(&agent_id, target.0, expected_fingerprint.0, auth)
                .await?;
            Ok(Json(RevertWorkerResponse {}))
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    /// Fork an agent
    ///
    /// Creates a new agent of the same type with the oplog of this agent up to the provided
    /// index, and returns the ID of the new agent.
    #[oai(
        path = "/:environment_id/agents/:agent_id/fork",
        method = "post",
        operation_id = "fork_worker"
    )]
    async fn fork_worker(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        request: Json<ForkWorkerRequest>,
        #[oai(name = "expectedFingerprint")] expected_fingerprint: Query<Option<AgentFingerprint>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ForkWorkerResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = OwnedAgentId::new(environment_id.0, &agent_id.0);
        let record = recorded_http_api_request!("fork_worker", agent_id = agent_id.to_string());

        let response = async {
            let forked_agent_id = self
                .worker_service
                .fork_worker(
                    &agent_id,
                    None,
                    request.0.oplog_index_cutoff,
                    None,
                    expected_fingerprint.0,
                    auth,
                )
                .await?;
            Ok(Json(ForkWorkerResponse {
                agent_id: forked_agent_id,
            }))
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    /// Connect to an agent using a websocket and stream events
    #[oai(
        path = "/:environment_id/agents/:agent_id/connect",
        method = "get",
        operation_id = "worker_connect"
    )]
    pub async fn worker_connect(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_id: Path<AgentId>,
        #[oai(name = "expectedFingerprint")] expected_fingerprint: Query<Option<AgentFingerprint>>,
        websocket: WebSocket,
        token: WrappedGolemSecuritySchema,
    ) -> Result<BoxWebSocketUpgraded> {
        let agent_id = OwnedAgentId::new(environment_id.0, &agent_id.0);
        let worker_stream = self
            .connect_to_worker(&agent_id, expected_fingerprint.0, token.0.secret())
            .await?;

        // The stream captured this when it was built, inside the request span.
        let origin = worker_stream.origin();
        let agent_id = agent_id.agent_id;

        let upgraded: BoxWebSocketUpgraded = websocket
            .config(self.connect_websocket.websocket_config())
            .on_upgrade(Box::new(move |socket_stream| {
                Box::pin(async move {
                    let _active = ActiveHttpApiStream::open(HttpApiStreamKind::Connect);
                    let (sink, stream) = socket_stream.split();
                    let _ = proxy_worker_connection(
                        agent_id,
                        origin,
                        worker_stream,
                        sink,
                        stream,
                        WORKER_CONNECT_PING_INTERVAL,
                        WORKER_CONNECT_PING_TIMEOUT,
                    )
                    .await;
                })
            }));

        Ok(upgraded)
    }

    async fn connect_to_worker(
        &self,
        agent_id: &OwnedAgentId,
        expected_fingerprint: Option<AgentFingerprint>,
        token: TokenSecret,
    ) -> Result<ConnectWorkerStream> {
        let auth = self.auth_service.authenticate_token(token).await?;
        let record = recorded_http_api_request!("connect_worker", agent_id = agent_id.to_string());

        let response = self
            .worker_service
            .connect(agent_id, expected_fingerprint, auth)
            .instrument(record.span.clone())
            .await
            .map_err(Into::into);

        record.result(response)
    }
}

fn make_component_file_path(name: String) -> Result<CanonicalFilePath> {
    CanonicalFilePath::from_rel_str(&name).map_err(|error| {
        ApiEndpointError::bad_request(
            api::error_code::INVALID_FILE_NAME,
            golem_common::safe(format!("Invalid file name: {error}")),
        )
    })
}

/// The scan allowance of a range read, from the paired `scanEntries` and `scanMillis` parameters.
/// Only a range read, a read with `to` and without `query`, takes an allowance.
fn oplog_scan_allowance(
    query: Option<&str>,
    to: Option<u64>,
    scan_entries: Option<u64>,
    scan_millis: Option<u64>,
) -> Result<Option<OplogScanAllowance>> {
    let invalid = |message: &str| {
        ApiEndpointError::bad_request(
            api::error_code::INVALID_OPLOG_QUERY_PARAMS,
            golem_common::safe(message.to_string()),
        )
    };
    match (scan_entries, scan_millis, to, query) {
        (None, None, _, _) => Ok(None),
        (Some(entries), Some(millis), Some(_), None) => {
            Ok(Some(OplogScanAllowance { entries, millis }))
        }
        (Some(_), Some(_), _, Some(_)) => Err(invalid(
            "Cannot specify both the 'scanEntries' and 'scanMillis' parameters and the 'query' parameter",
        )),
        (Some(_), Some(_), None, None) => Err(invalid(
            "The 'scanEntries' and 'scanMillis' parameters need the 'to' parameter",
        )),
        _ => Err(invalid(
            "The 'scanEntries' and 'scanMillis' parameters go together",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{WorkerApi, oplog_scan_allowance};
    use crate::api::common::ApiEndpointError;
    use crate::mcp::OplogRead;
    use golem_common::model::OwnedAgentId;
    use golem_common::model::agent::{AgentKey, AgentMode};
    use golem_common::model::oplog::{OplogIndex, OplogRange, OplogScanAllowance};
    use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};
    use golem_service_base::model::auth::AuthCtx;
    use test_r::test;

    #[test]
    fn a_scan_allowance_needs_both_parameters_and_a_range_read() {
        let allowance = OplogScanAllowance {
            entries: 7,
            millis: 9,
        };
        let needs_to = "The 'scanEntries' and 'scanMillis' parameters need the 'to' parameter";
        let no_query = "Cannot specify both the 'scanEntries' and 'scanMillis' parameters and the 'query' parameter";
        let together = "The 'scanEntries' and 'scanMillis' parameters go together";
        [
            ((None, None, None, None), Ok(None)),
            ((None, Some(9), None, None), Ok(None)),
            ((Some("error"), Some(9), None, None), Ok(None)),
            ((None, Some(9), Some(7), Some(9)), Ok(Some(allowance))),
            ((None, None, Some(7), Some(9)), Err(needs_to)),
            ((Some("error"), None, Some(7), Some(9)), Err(no_query)),
            ((Some("error"), Some(9), Some(7), Some(9)), Err(no_query)),
            ((None, Some(9), Some(7), None), Err(together)),
            ((None, Some(9), None, Some(9)), Err(together)),
        ]
        .into_iter()
        .for_each(|((query, to, entries, millis), expected)| {
            assert_eq!(
                oplog_scan_allowance(query, to, entries, millis).map_err(|error| match error {
                    ApiEndpointError::BadRequest(body) => body.0.errors.join(" "),
                    _ => "not a bad request".to_string(),
                }),
                expected.map_err(str::to_string),
                "query {query:?}, to {to:?}, scanEntries {entries:?}, scanMillis {millis:?}"
            )
        });
    }

    #[test]
    async fn an_oplog_query_reads_its_range_and_a_read_without_a_query_reads_its_range() {
        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let (worker_service, auth_service) = harness.rest_api_services();
        let api = WorkerApi::new(worker_service, auth_service, Default::default());
        let key = AgentKey::new(
            harness.agent_type.type_name.clone(),
            TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::record(vec![])),
                SchemaValue::Record { fields: vec![] },
            ),
        )
        .unwrap();
        let agent_id = OwnedAgentId::new(harness.environment_id, &harness.add_keyed_agent(&key));
        let index = OplogIndex::from_u64;
        let allowance = OplogScanAllowance {
            entries: 7,
            millis: 9,
        };
        let read = |from, to, query: Option<&str>, allowance| {
            api.get_oplog_internal(
                agent_id.clone(),
                from,
                to,
                50,
                None,
                query.map(str::to_string),
                None,
                allowance,
                Default::default(),
                AuthCtx::system(),
            )
        };

        read(Some(3), Some(9), Some("error"), None).await.unwrap();
        read(Some(4), None, None, None).await.unwrap();
        // A read without `from` keeps its start open, so that the executor starts it at the first
        // entry that the oplog holds. The scan allowance of a range read goes to the executor.
        read(None, Some(9), None, Some(allowance)).await.unwrap();
        assert!(read(Some(5), Some(4), Some("error"), None).await.is_err());

        assert_eq!(
            *harness.oplog_reads.lock().unwrap(),
            vec![
                OplogRead {
                    range: OplogRange::new(Some(index(3)), Some(index(9))).unwrap(),
                    query: Some("error".to_string()),
                    allowance: None,
                },
                OplogRead {
                    range: OplogRange::new(Some(index(4)), None).unwrap(),
                    query: None,
                    allowance: None,
                },
                OplogRead {
                    range: OplogRange::new(None, Some(index(9))).unwrap(),
                    query: None,
                    allowance: Some(allowance),
                },
            ]
        );
    }
}
