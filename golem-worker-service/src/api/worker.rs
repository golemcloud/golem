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
use crate::model;
use crate::service::auth::AuthService;
use crate::service::component::ComponentService;
use crate::service::worker::ConnectWorkerStream;
use crate::service::worker::{WorkerService, proxy_worker_connection};
use futures::StreamExt;
use futures::TryStreamExt;
use golem_common::base_model::api;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::{CanonicalFilePath, ComponentId, PluginPriority};
use golem_common::model::oplog::OplogCursor;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::worker::{AgentCreationRequest, AgentMetadataDto, RevertWorkerTarget};
use golem_common::model::{AgentFilter, AgentId, IdempotencyKey, ScanCursor};
use golem_common::{SafeDisplay, recorded_http_api_request};
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::{
    AuthCtx, EnvironmentAction, GolemSecurityScheme, WrappedGolemSecuritySchema,
};
use golem_service_base::model::component::Component;
use golem_service_base::model::*;
use poem::Body;
use poem::web::websocket::{BoxWebSocketUpgraded, WebSocket};
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Binary, Json};
use poem_openapi::*;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing::Instrument;

const WORKER_CONNECT_PING_INTERVAL: Duration = Duration::from_secs(30);
const WORKER_CONNECT_PING_TIMEOUT: Duration = Duration::from_secs(15);

type Result<T> = std::result::Result<T, ApiEndpointError>;

pub struct WorkerApi {
    component_service: Arc<dyn ComponentService>,
    worker_service: Arc<WorkerService>,
    auth_service: Arc<dyn AuthService>,
}

#[OpenApi(prefix_path = "/v1/components", tag = ApiTags::Worker)]
impl WorkerApi {
    pub fn new(
        component_service: Arc<dyn ComponentService>,
        worker_service: Arc<WorkerService>,
        auth_service: Arc<dyn AuthService>,
    ) -> Self {
        Self {
            component_service,
            worker_service,
            auth_service,
        }
    }

    /// Launch a new worker.
    ///
    /// Creates a new worker. The worker initially is in `Idle`` status, waiting to be invoked.
    ///
    /// The parameters in the request are the following:
    /// - `name` is the name of the created worker. This has to be unique, but only for a given component
    /// - `args` is a list of strings which appear as command line arguments for the worker
    /// - `env` is a list of key-value pairs (represented by arrays) which appear as environment variables for the worker
    #[oai(
        path = "/:component_id/workers",
        method = "post",
        operation_id = "launch_new_worker"
    )]
    async fn launch_new_worker(
        &self,
        component_id: Path<ComponentId>,
        request: Json<AgentCreationRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<WorkerCreationResponse>> {
        let record = recorded_http_api_request!(
            "launch_new_worker",
            component_id = component_id.0.to_string(),
            name = request.name
        );

        let response = self
            .launch_new_worker_internal(component_id.0, request.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn launch_new_worker_internal(
        &self,
        component_id: ComponentId,
        request: AgentCreationRequest,
        token: GolemSecurityScheme,
    ) -> Result<Json<WorkerCreationResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let AgentCreationRequest {
            name,
            env,
            wasi_config,
            ..
        } = request;

        let (agent_id, component) = self
            .normalize_agent_id_by_latest_version(component_id, &name)
            .await?;

        let component_revision = self
            .worker_service
            .create_with_component(
                &agent_id,
                component,
                env,
                wasi_config,
                request.config,
                false,
                auth,
                None,
                None,
            )
            .await?;

        Ok(Json(WorkerCreationResponse {
            agent_id,
            component_revision,
        }))
    }

    /// Delete a worker
    ///
    /// Interrupts and deletes an existing worker.
    #[oai(
        path = "/:component_id/workers/:agent_name",
        method = "delete",
        operation_id = "delete_worker"
    )]
    async fn delete_worker(
        &self,
        component_id: Path<ComponentId>,
        agent_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<DeleteWorkerResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let agent_id = self
            .normalize_agent_id(component_id.0, agent_name.as_str())
            .await?;

        let record = recorded_http_api_request!("delete_worker", agent_id = agent_id.to_string(),);

        let response = self
            .delete_worker_internal(agent_id, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_worker_internal(
        &self,
        agent_id: AgentId,
        auth: AuthCtx,
    ) -> Result<Json<DeleteWorkerResponse>> {
        self.worker_service.delete(&agent_id, auth).await?;
        Ok(Json(DeleteWorkerResponse {}))
    }

    /// Complete a promise
    ///
    /// Completes a promise with a given custom array of bytes.
    /// The promise must be previously created from within the worker, and it's identifier (a combination of a worker identifier and an oplogIdx ) must be sent out to an external caller so it can use this endpoint to mark the promise completed.
    /// The data field is sent back to the worker, and it has no predefined meaning.
    #[oai(
        path = "/:component_id/workers/:agent_name/complete",
        method = "post",
        operation_id = "complete_promise"
    )]
    async fn complete_promise(
        &self,
        component_id: Path<ComponentId>,
        agent_name: Path<String>,
        params: Json<CompleteParameters>,
        token: GolemSecurityScheme,
    ) -> Result<Json<bool>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let agent_id = self
            .normalize_agent_id(component_id.0, agent_name.as_str())
            .await?;

        let record =
            recorded_http_api_request!("complete_promise", agent_id = agent_id.to_string());

        let response = self
            .complete_promise_internal(agent_id, params.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn complete_promise_internal(
        &self,
        agent_id: AgentId,
        params: CompleteParameters,
        auth: AuthCtx,
    ) -> Result<Json<bool>> {
        let CompleteParameters { oplog_idx, data } = params;

        let response = self
            .worker_service
            .complete_promise(&agent_id, oplog_idx, data, auth)
            .await?;

        Ok(Json(response))
    }

    /// Interrupt a worker
    ///
    /// Interrupts the execution of a worker.
    /// The worker's status will be Interrupted unless the recover-immediately parameter was used, in which case it remains as it was.
    /// An interrupted worker can be still used, and it is going to be automatically resumed the first time it is used.
    /// For example in case of a new invocation, the previously interrupted invocation is continued before the new one gets processed.
    #[oai(
        path = "/:component_id/workers/:agent_name/interrupt",
        method = "post",
        operation_id = "interrupt_worker"
    )]
    async fn interrupt_worker(
        &self,
        component_id: Path<ComponentId>,
        agent_name: Path<String>,
        /// if true will simulate a worker recovery. Defaults to false.
        #[oai(name = "recovery-immediately")]
        recover_immediately: Query<Option<bool>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<InterruptResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let agent_id = self
            .normalize_agent_id(component_id.0, agent_name.as_str())
            .await?;

        let record =
            recorded_http_api_request!("interrupt_worker", agent_id = agent_id.to_string());

        let response = self
            .interrupt_worker_internal(agent_id, recover_immediately.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn interrupt_worker_internal(
        &self,
        agent_id: AgentId,
        recover_immediately: Option<bool>,
        auth: AuthCtx,
    ) -> Result<Json<InterruptResponse>> {
        self.worker_service
            .interrupt(&agent_id, recover_immediately.unwrap_or(false), auth)
            .await?;

        Ok(Json(InterruptResponse {}))
    }

    /// Get metadata of a worker
    ///
    /// Returns metadata about an existing worker:
    /// - `workerId` is a combination of the used component and the worker's user specified name
    /// - `accountId` the account the worker is created by
    /// - `args` is the provided command line arguments passed to the worker
    /// - `env` is the provided map of environment variables passed to the worker
    /// - `componentVersion` is the version of the component used by the worker
    /// - `retryCount` is the number of retries the worker did in case of a failure
    /// - `status` is the worker's current status, one of the following:
    ///     - `Running` if the worker is currently executing
    ///     - `Idle` if the worker is waiting for an invocation
    ///     - `Suspended` if the worker was running but is now waiting to be resumed by an event (such as end of a sleep, a promise, etc)
    ///     - `Interrupted` if the worker was interrupted by the user
    ///     - `Retrying` if the worker failed, and an automatic retry was scheduled for it
    ///     - `Failed` if the worker failed and there are no more retries scheduled for it
    ///     - `Exited` if the worker explicitly exited using the exit WASI function
    #[oai(
        path = "/:component_id/workers/:agent_name",
        method = "get",
        operation_id = "get_worker_metadata"
    )]
    async fn get_worker_metadata(
        &self,
        component_id: Path<ComponentId>,
        agent_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<AgentMetadataDto>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let agent_id = self
            .normalize_agent_id(component_id.0, agent_name.as_str())
            .await?;

        let record =
            recorded_http_api_request!("get_worker_metadata", agent_id = agent_id.to_string());

        let response = self
            .get_worker_metadata_internal(agent_id, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_worker_metadata_internal(
        &self,
        agent_id: AgentId,
        auth: AuthCtx,
    ) -> Result<Json<AgentMetadataDto>> {
        let response = self.worker_service.get_metadata(&agent_id, auth).await?;

        Ok(Json(response))
    }

    /// Get metadata of multiple workers
    ///
    /// ### Filters
    ///
    /// | Property    | Comparator             | Description                    | Example                         |
    /// |-------------|------------------------|--------------------------------|----------------------------------|
    /// | name        | StringFilterComparator | Name of worker                 | `name = worker-name`             |
    /// | version     | FilterComparator       | Version of worker              | `version >= 0`                   |
    /// | status      | FilterComparator       | Status of worker               | `status = Running`               |
    /// | env.\[key\] | StringFilterComparator | Environment variable of worker | `env.var1 = value`               |
    /// | createdAt   | FilterComparator       | Creation time of worker        | `createdAt > 2024-04-01T12:10:00Z` |
    ///
    ///
    /// ### Comparators
    ///
    /// - StringFilterComparator: `eq|equal|=|==`, `ne|notequal|!=`, `like`, `notlike`
    /// - FilterComparator: `eq|equal|=|==`, `ne|notequal|!=`, `ge|greaterequal|>=`, `gt|greater|>`, `le|lessequal|<=`, `lt|less|<`
    ///
    /// Returns metadata about an existing component workers:
    /// - `workers` list of workers metadata
    /// - `cursor` cursor for next request, if cursor is empty/null, there are no other values
    #[oai(
        path = "/:component_id/workers",
        method = "get",
        operation_id = "get_workers_metadata"
    )]
    async fn get_workers_metadata(
        &self,
        component_id: Path<ComponentId>,
        /// Filter for worker metadata in form of `property op value`. Can be used multiple times (AND condition is applied between them)
        filter: Query<Option<Vec<String>>>,
        /// Count of listed values, default: 50
        cursor: Query<Option<String>>,
        /// Position where to start listing, if not provided, starts from the beginning. It is used to get the next page of results. To get next page, use the cursor returned in the response
        count: Query<Option<u64>>,
        /// Precision in relation to worker status, if true, calculate the most up-to-date status for each worker, default is false
        precise: Query<Option<bool>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<model::WorkersMetadataResponse>> {
        let record = recorded_http_api_request!(
            "get_workers_metadata",
            component_id = component_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_workers_metadata_internal(
                component_id.0,
                filter.0,
                cursor.0,
                count.0,
                precise.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_workers_metadata_internal(
        &self,
        component_id: ComponentId,
        filter: Option<Vec<String>>,
        cursor: Option<String>,
        count: Option<u64>,
        precise: Option<bool>,
        auth: AuthCtx,
    ) -> Result<Json<model::WorkersMetadataResponse>> {
        let filter = match filter {
            Some(filters) if !filters.is_empty() => {
                Some(AgentFilter::from(filters).map_err(|e| {
                    ApiEndpointError::bad_request(
                        api::error_code::INVALID_AGENT_FILTER,
                        golem_common::safe(e),
                    )
                })?)
            }
            _ => None,
        };

        let cursor = match cursor {
            Some(cursor) => Some(ScanCursor::from_str(&cursor).map_err(|e| {
                ApiEndpointError::bad_request(
                    api::error_code::INVALID_SCAN_CURSOR,
                    golem_common::safe(e),
                )
            })?),
            None => None,
        };

        let (cursor, workers) = self
            .worker_service
            .find_metadata(
                component_id,
                filter,
                cursor.unwrap_or_default(),
                count.unwrap_or(50),
                precise.unwrap_or(false),
                auth,
            )
            .await?;

        Ok(Json(model::WorkersMetadataResponse { workers, cursor }))
    }

    /// Advanced search for workers
    ///
    /// ### Filter types
    /// | Type      | Comparator             | Description                    | Example                                                                                       |
    /// |-----------|------------------------|--------------------------------|-----------------------------------------------------------------------------------------------|
    /// | Name      | StringFilterComparator | Name of worker                 | `{ "type": "Name", "comparator": "Equal", "value": "worker-name" }`                           |
    /// | Version   | FilterComparator       | Version of worker              | `{ "type": "Version", "comparator": "GreaterEqual", "value": 0 }`                             |
    /// | Status    | FilterComparator       | Status of worker               | `{ "type": "Status", "comparator": "Equal", "value": "Running" }`                             |
    /// | Env       | StringFilterComparator | Environment variable of worker | `{ "type": "Env", "name": "var1", "comparator": "Equal", "value": "value" }`                  |
    /// | CreatedAt | FilterComparator       | Creation time of worker        | `{ "type": "CreatedAt", "comparator": "Greater", "value": "2024-04-01T12:10:00Z" }`           |
    /// | And       |                        | And filter combinator          | `{ "type": "And", "filters": [ ... ] }`                                                       |
    /// | Or        |                        | Or filter combinator           | `{ "type": "Or", "filters": [ ... ] }`                                                        |
    /// | Not       |                        | Negates the specified filter   | `{ "type": "Not", "filter": { "type": "Version", "comparator": "GreaterEqual", "value": 0 } }`|
    ///
    /// ### Comparators
    /// - StringFilterComparator: `Equal`, `NotEqual`, `Like`, `NotLike`
    /// - FilterComparator: `Equal`, `NotEqual`, `GreaterEqual`, `Greater`, `LessEqual`, `Less`
    ///
    /// Returns metadata about an existing component workers:
    /// - `workers` list of workers metadata
    /// - `cursor` cursor for next request, if cursor is empty/null, there are no other values
    #[oai(
        path = "/:component_id/workers/find",
        method = "post",
        operation_id = "find_workers_metadata"
    )]
    async fn find_workers_metadata(
        &self,
        component_id: Path<ComponentId>,
        params: Json<WorkersMetadataRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<model::WorkersMetadataResponse>> {
        let record = recorded_http_api_request!(
            "find_workers_metadata",
            component_id = component_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .find_workers_metadata_internal(component_id.0, params.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn find_workers_metadata_internal(
        &self,
        component_id: ComponentId,
        params: WorkersMetadataRequest,
        auth: AuthCtx,
    ) -> Result<Json<model::WorkersMetadataResponse>> {
        let (cursor, workers) = self
            .worker_service
            .find_metadata(
                component_id,
                params.filter.clone(),
                params.cursor.clone().unwrap_or_default(),
                params.count.unwrap_or(50),
                params.precise.unwrap_or(false),
                auth,
            )
            .await?;

        Ok(Json(model::WorkersMetadataResponse { workers, cursor }))
    }

    /// Resume a worker
    #[oai(
        path = "/:component_id/workers/:agent_name/resume",
        method = "post",
        operation_id = "resume_worker"
    )]
    async fn resume_worker(
        &self,
        component_id: Path<ComponentId>,
        agent_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ResumeResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let agent_id = self
            .normalize_agent_id(component_id.0, agent_name.as_str())
            .await?;

        let record = recorded_http_api_request!("resume_worker", agent_id = agent_id.to_string());

        let response = self
            .resume_worker_internal(agent_id, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn resume_worker_internal(
        &self,
        agent_id: AgentId,
        auth: AuthCtx,
    ) -> Result<Json<ResumeResponse>> {
        self.worker_service.resume(&agent_id, false, auth).await?;

        Ok(Json(ResumeResponse {}))
    }

    /// Update a worker
    #[oai(
        path = "/:component_id/workers/:agent_name/update",
        method = "post",
        operation_id = "update_worker"
    )]
    async fn update_worker(
        &self,
        component_id: Path<ComponentId>,
        agent_name: Path<String>,
        params: Json<UpdateWorkerRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<UpdateWorkerResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let agent_id = self
            .normalize_agent_id(component_id.0, agent_name.as_str())
            .await?;

        let record = recorded_http_api_request!("update_worker", agent_id = agent_id.to_string());

        let response = self
            .update_worker_internal(agent_id, params.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_worker_internal(
        &self,
        agent_id: AgentId,
        params: UpdateWorkerRequest,
        auth: AuthCtx,
    ) -> Result<Json<UpdateWorkerResponse>> {
        self.worker_service
            .update(
                &agent_id,
                params.mode,
                params.target_revision,
                params.disable_wakeup,
                auth,
            )
            .await?;

        Ok(Json(UpdateWorkerResponse {}))
    }

    /// Get the oplog of a worker
    #[oai(
        path = "/:component_id/workers/:agent_name/oplog",
        method = "get",
        operation_id = "get_oplog"
    )]
    async fn get_oplog(
        &self,
        component_id: Path<ComponentId>,
        agent_name: Path<String>,
        from: Query<Option<u64>>,
        count: Query<u64>,
        cursor: Query<Option<OplogCursor>>,
        query: Query<Option<String>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<GetOplogResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let agent_id = self
            .normalize_agent_id(component_id.0, agent_name.as_str())
            .await?;

        let record = recorded_http_api_request!("get_oplog", agent_id = agent_id.to_string());

        let response = self
            .get_oplog_internal(agent_id, from.0, count.0, cursor.0, query.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_oplog_internal(
        &self,
        agent_id: AgentId,
        from: Option<u64>,
        count: u64,
        cursor: Option<OplogCursor>,
        query: Option<String>,
        auth: AuthCtx,
    ) -> Result<Json<GetOplogResponse>> {
        let response = match (from, query) {
            (Some(_), Some(_)) => {
                return Err(ApiEndpointError::bad_request(
                    api::error_code::INVALID_OPLOG_QUERY_PARAMS,
                    golem_common::safe(
                        "Cannot specify both the 'from' and the 'query' parameters".to_string(),
                    ),
                ));
            }
            (Some(from), None) => {
                self.worker_service
                    .get_oplog(&agent_id, OplogIndex::from_u64(from), cursor, count, auth)
                    .await?
            }
            (None, Some(query)) => {
                self.worker_service
                    .search_oplog(&agent_id, cursor, count, query, auth)
                    .await?
            }
            (None, None) => {
                self.worker_service
                    .get_oplog(&agent_id, OplogIndex::INITIAL, cursor, count, auth)
                    .await?
            }
        };

        Ok(Json(response))
    }

    /// List files in a worker
    #[oai(
        path = "/:component_id/workers/:agent_name/files/:file_name",
        method = "get",
        operation_id = "get_files"
    )]
    async fn get_file(
        &self,
        component_id: Path<ComponentId>,
        agent_name: Path<String>,
        file_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<GetFilesResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let agent_id = self
            .normalize_agent_id(component_id.0, agent_name.as_str())
            .await?;

        let record = recorded_http_api_request!("get_file", agent_id = agent_id.to_string());

        let response = self
            .get_file_internal(agent_id, file_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_file_internal(
        &self,
        agent_id: AgentId,
        file_name: String,
        auth: AuthCtx,
    ) -> Result<Json<GetFilesResponse>> {
        let path = make_component_file_path(file_name)?;

        let nodes = self
            .worker_service
            .get_file_system_node(&agent_id, path, auth)
            .await?;

        Ok(Json(GetFilesResponse {
            nodes: nodes.into_iter().map(|n| n.into()).collect(),
        }))
    }

    /// Get contents of a file in a worker
    #[oai(
        path = "/:component_id/workers/:agent_name/file-contents/:file_name",
        method = "get",
        operation_id = "get_file_content"
    )]
    async fn get_file_content(
        &self,
        component_id: Path<ComponentId>,
        agent_name: Path<String>,
        file_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Binary<Body>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let agent_id = self
            .normalize_agent_id(component_id.0, agent_name.as_str())
            .await?;

        let record = recorded_http_api_request!("get_files", agent_id = agent_id.to_string());

        let response = self
            .get_file_content_internal(agent_id, file_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_file_content_internal(
        &self,
        agent_id: AgentId,
        file_name: String,
        auth: AuthCtx,
    ) -> Result<Binary<Body>> {
        let path = make_component_file_path(file_name)?;

        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::ViewWorker,
                &auth,
            )
            .await?;

        let bytes = self
            .worker_service
            .get_file_contents(&agent_id, path, auth)
            .await?;

        Ok(Binary(Body::from_bytes_stream(
            bytes.map_err(|e| std::io::Error::other(e.to_string())),
        )))
    }

    /// Activate a plugin
    ///
    /// The plugin must be one of the installed plugins for the worker's current component version.
    #[oai(
        path = "/:component_id/workers/:agent_name/activate-plugin",
        method = "post",
        operation_id = "activate_plugin"
    )]
    async fn activate_plugin(
        &self,
        component_id: Path<ComponentId>,
        agent_name: Path<String>,
        #[oai(name = "plugin-priority")] plugin_installation_id: Query<PluginPriority>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ActivatePluginResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let agent_id = self
            .normalize_agent_id(component_id.0, agent_name.as_str())
            .await?;

        let record = recorded_http_api_request!(
            "activate_plugin",
            agent_id = agent_id.to_string(),
            plugin_installation_id = plugin_installation_id.to_string()
        );

        let response = self
            .activate_plugin_internal(agent_id, plugin_installation_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn activate_plugin_internal(
        &self,
        agent_id: AgentId,
        plugin_priority: PluginPriority,
        auth: AuthCtx,
    ) -> Result<Json<ActivatePluginResponse>> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth,
            )
            .await?;

        self.worker_service
            .activate_plugin(&agent_id, plugin_priority, auth)
            .await?;

        Ok(Json(ActivatePluginResponse {}))
    }

    /// Deactivate a plugin
    ///
    /// The plugin must be one of the installed plugins for the worker's current component version.
    #[oai(
        path = "/:component_id/workers/:agent_name/deactivate-plugin",
        method = "post",
        operation_id = "deactivate_plugin"
    )]
    async fn deactivate_plugin(
        &self,
        component_id: Path<ComponentId>,
        agent_name: Path<String>,
        #[oai(name = "plugin-priority")] plugin_priority: Query<PluginPriority>,
        token: GolemSecurityScheme,
    ) -> Result<Json<DeactivatePluginResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let agent_id = self
            .normalize_agent_id(component_id.0, agent_name.as_str())
            .await?;

        let record = recorded_http_api_request!(
            "activate_plugin",
            agent_id = agent_id.to_string(),
            plugin_priority = plugin_priority.to_string()
        );

        let response = self
            .deactivate_plugin_internal(agent_id, plugin_priority.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn deactivate_plugin_internal(
        &self,
        agent_id: AgentId,
        plugin_priority: PluginPriority,
        auth: AuthCtx,
    ) -> Result<Json<DeactivatePluginResponse>> {
        self.worker_service
            .deactivate_plugin(&agent_id, plugin_priority, auth)
            .await?;

        Ok(Json(DeactivatePluginResponse {}))
    }

    /// Revert a worker
    ///
    /// Reverts a worker by undoing either the last few invocations or the last few recorded oplog entries.
    #[oai(
        path = "/:component_id/workers/:agent_name/revert",
        method = "post",
        operation_id = "revert_worker"
    )]
    async fn revert_worker(
        &self,
        component_id: Path<ComponentId>,
        agent_name: Path<String>,
        target: Json<RevertWorkerTarget>,
        token: GolemSecurityScheme,
    ) -> Result<Json<RevertWorkerResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let agent_id = self
            .normalize_agent_id(component_id.0, agent_name.as_str())
            .await?;

        let record = recorded_http_api_request!("revert_worker", agent_id = agent_id.to_string(),);

        let response = self
            .revert_worker_internal(agent_id, target.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn revert_worker_internal(
        &self,
        agent_id: AgentId,
        target: RevertWorkerTarget,
        auth: AuthCtx,
    ) -> Result<Json<RevertWorkerResponse>> {
        self.worker_service
            .revert_worker(&agent_id, target, auth)
            .await?;

        Ok(Json(RevertWorkerResponse {}))
    }

    /// Fork a worker
    ///
    /// Fork a worker by creating a new worker with the oplog up to the provided index
    #[oai(
        path = "/:component_id/workers/:agent_name/fork",
        method = "post",
        operation_id = "fork_worker"
    )]
    async fn fork_worker(
        &self,
        component_id: Path<ComponentId>,
        agent_name: Path<String>,
        request: Json<ForkWorkerRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ForkWorkerResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let agent_id = self
            .normalize_agent_id(component_id.0, agent_name.as_str())
            .await?;

        let record = recorded_http_api_request!("revert_worker", agent_id = agent_id.to_string(),);

        let response = self
            .fork_worker_internal(agent_id, request.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn fork_worker_internal(
        &self,
        agent_id: AgentId,
        request: ForkWorkerRequest,
        auth: AuthCtx,
    ) -> Result<Json<ForkWorkerResponse>> {
        self.worker_service
            .fork_worker(
                &agent_id,
                &request.target_agent_id,
                request.oplog_index_cutoff,
                auth,
            )
            .await?;

        Ok(Json(ForkWorkerResponse {}))
    }

    /// Cancels a pending invocation if it has not started yet
    ///
    /// The invocation to be cancelled is identified by the idempotency key passed to the invoke API.
    #[oai(
        path = "/:component_id/workers/:agent_name/invocations/:idempotency_key",
        method = "delete",
        operation_id = "cancel_invocation"
    )]
    async fn cancel_invocation(
        &self,
        component_id: Path<ComponentId>,
        agent_name: Path<String>,
        idempotency_key: Path<IdempotencyKey>,
        token: GolemSecurityScheme,
    ) -> Result<Json<CancelInvocationResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let agent_id = self
            .normalize_agent_id(component_id.0, agent_name.as_str())
            .await?;

        let record = recorded_http_api_request!(
            "cancel_invocation",
            agent_id = agent_id.to_string(),
            idempotency_key = idempotency_key.0.to_string(),
        );

        let response = self
            .cancel_invocation_internal(agent_id, idempotency_key.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn cancel_invocation_internal(
        &self,
        agent_id: AgentId,
        idempotency_key: IdempotencyKey,
        auth: AuthCtx,
    ) -> Result<Json<CancelInvocationResponse>> {
        let canceled = self
            .worker_service
            .cancel_invocation(&agent_id, &idempotency_key, auth)
            .await?;

        Ok(Json(CancelInvocationResponse { canceled }))
    }

    /// Connect to a worker using a websocket and stream events
    #[oai(
        path = "/:component_id/workers/:agent_name/connect",
        method = "get",
        operation_id = "worker_connect"
    )]
    pub async fn worker_connect(
        &self,
        component_id: Path<ComponentId>,
        agent_name: Path<String>,
        websocket: WebSocket,
        token: WrappedGolemSecuritySchema,
    ) -> Result<BoxWebSocketUpgraded> {
        let (agent_id, worker_stream) = self
            .connect_to_worker(component_id.0, agent_name.0, token.0.secret())
            .await?;

        let upgraded: BoxWebSocketUpgraded = websocket.on_upgrade(Box::new(|socket_stream| {
            Box::pin(async move {
                let (sink, stream) = socket_stream.split();
                let _ = proxy_worker_connection(
                    agent_id,
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
        component_id: ComponentId,
        agent_name: String,
        token: TokenSecret,
    ) -> Result<(AgentId, ConnectWorkerStream)> {
        let auth = self.auth_service.authenticate_token(token).await?;

        let agent_id = self
            .normalize_agent_id(component_id, agent_name.as_str())
            .await?;

        let record = recorded_http_api_request!("connect_worker", agent_id = agent_id.to_string());

        let response = self
            .connect_to_worker_internal(agent_id.clone(), auth)
            .instrument(record.span.clone())
            .await
            .map(|stream| (agent_id, stream));

        record.result(response)
    }

    async fn connect_to_worker_internal(
        &self,
        agent_id: AgentId,
        auth: AuthCtx,
    ) -> Result<ConnectWorkerStream> {
        let stream = self.worker_service.connect(&agent_id, auth).await?;
        Ok(stream)
    }

    async fn normalize_agent_id_by_latest_version(
        &self,
        component_id: ComponentId,
        agent_id: &str,
    ) -> Result<(AgentId, Component)> {
        let latest_component = self
            .component_service
            .get_latest_by_id_uncached(component_id)
            .await
            .map_err(|error| {
                ApiEndpointError::not_found(
                    api::error_code::COMPONENT_NOT_FOUND,
                    golem_common::safe(format!(
                        "Couldn't retrieve the component: {}. error: {}",
                        &component_id,
                        error.to_safe_string()
                    )),
                )
            })?;

        let agent_id =
            normalize_agent_name_with_latest_component(component_id, agent_id, &latest_component)?;
        Ok((agent_id, latest_component))
    }

    async fn normalize_agent_id(
        &self,
        component_id: ComponentId,
        agent_id: &str,
    ) -> Result<AgentId> {
        AgentId::from_agent_name_string(component_id, agent_id).map_err(|error| {
            ApiEndpointError::bad_request(
                api::error_code::INVALID_WORKER_ID,
                golem_common::safe(format!("Invalid worker id: {error}")),
            )
        })
    }
}

fn normalize_agent_name_with_latest_component(
    component_id: ComponentId,
    agent_id: &str,
    latest_component: &Component,
) -> Result<AgentId> {
    if latest_component.metadata.is_agent()
        && let Ok((parsed_agent_id, agent_type)) =
            ParsedAgentId::parse_and_resolve_type(agent_id, &latest_component.metadata)
    {
        let normalized_agent_id = ParsedAgentId::new_auto_phantom(
            parsed_agent_id.agent_type,
            parsed_agent_id.parameters,
            parsed_agent_id.phantom_id,
            agent_type.mode,
        )
        .map_err(|error| {
            ApiEndpointError::bad_request(
                api::error_code::INVALID_WORKER_ID,
                golem_common::safe(format!("Invalid worker id: {error}")),
            )
        })?;

        return Ok(AgentId {
            component_id,
            agent_id: normalized_agent_id.to_string(),
        });
    }

    AgentId::from_agent_name_string(component_id, agent_id).map_err(|error| {
        ApiEndpointError::bad_request(
            api::error_code::INVALID_WORKER_ID,
            golem_common::safe(format!("Invalid worker id: {error}")),
        )
    })
}

fn make_component_file_path(name: String) -> Result<CanonicalFilePath> {
    CanonicalFilePath::from_rel_str(&name).map_err(|error| {
        ApiEndpointError::bad_request(
            api::error_code::INVALID_FILE_NAME,
            golem_common::safe(format!("Invalid file name: {error}")),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::normalize_agent_name_with_latest_component;
    use chrono::Utc;
    use golem_common::model::AgentId;
    use golem_common::model::Empty;
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::{
        AgentConstructor, AgentMethod, AgentMode, AgentType, AgentTypeName, DataSchema,
        NamedElementSchemas, Snapshotting,
    };
    use golem_common::model::application::ApplicationId;
    use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
    use golem_common::model::component_metadata::ComponentMetadata;
    use golem_common::model::diff::Hash;
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::model::component::Component;
    use test_r::test;
    use uuid::Uuid;

    fn test_agent_type(mode: AgentMode) -> AgentType {
        AgentType {
            type_name: AgentTypeName("weather-agent".to_string()),
            description: String::new(),
            source_language: String::new(),
            constructor: AgentConstructor {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas::empty()),
            },
            methods: vec![AgentMethod {
                name: "run".to_string(),
                description: String::new(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas::empty()),
                output_schema: DataSchema::Tuple(NamedElementSchemas::empty()),
                http_endpoint: vec![],
            }],
            dependencies: vec![],
            mode,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![],
        }
    }

    fn test_component(mode: AgentMode) -> Component {
        Component {
            id: ComponentId(Uuid::new_v4()),
            revision: ComponentRevision::INITIAL,
            environment_id: EnvironmentId(Uuid::new_v4()),
            component_name: ComponentName("weather-component".to_string()),
            hash: Hash::empty(),
            application_id: ApplicationId(Uuid::new_v4()),
            account_id: AccountId(Uuid::new_v4()),
            component_size: 0,
            metadata: ComponentMetadata::from_parts(
                vec![],
                vec![],
                None,
                None,
                vec![test_agent_type(mode)],
                std::collections::BTreeMap::new(),
            ),
            created_at: Utc::now(),
            wasm_hash: Hash::empty(),
            object_store_key: String::new(),
        }
    }

    fn phantom_id(agent_id: &AgentId) -> Option<Uuid> {
        agent_id
            .agent_id
            .rsplit_once('[')
            .and_then(|(_, phantom_id)| phantom_id.strip_suffix(']'))
            .and_then(|phantom_id| Uuid::parse_str(phantom_id).ok())
    }

    #[test]
    fn latest_component_normalization_auto_generates_phantom_for_ephemeral_agents() {
        let component = test_component(AgentMode::Ephemeral);

        let agent_id =
            normalize_agent_name_with_latest_component(component.id, "weather-agent()", &component)
                .unwrap();

        assert!(phantom_id(&agent_id).is_some());
    }

    #[test]
    fn latest_component_normalization_keeps_durable_agents_non_phantom() {
        let component = test_component(AgentMode::Durable);

        let agent_id =
            normalize_agent_name_with_latest_component(component.id, "weather-agent()", &component)
                .unwrap();

        assert!(phantom_id(&agent_id).is_none());
    }

    #[test]
    fn latest_component_normalization_preserves_non_agent_worker_names() {
        let component = test_component(AgentMode::Ephemeral);

        let agent_id =
            normalize_agent_name_with_latest_component(component.id, "custom-worker", &component)
                .unwrap();

        assert_eq!(agent_id.agent_id, "custom-worker");
    }
}
