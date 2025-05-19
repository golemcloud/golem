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

use crate::service::worker::WorkerService;
use futures::StreamExt;
use futures_util::TryStreamExt;
use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_common::model::error::{ErrorBody, ErrorsBody};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::public_oplog::OplogCursor;
use golem_common::model::{
    ComponentFilePath, ComponentId, IdempotencyKey, PluginInstallationId, ScanCursor,
    TargetWorkerId, WorkerFilter, WorkerId,
};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::auth::{DefaultNamespace, EmptyAuthCtx};
use golem_service_base::model::*;
use golem_worker_service_base::api::WorkerApiBaseError;
use golem_worker_service_base::empty_worker_metadata;
use golem_worker_service_base::http_invocation_context::grpc_invocation_context_from_request;
use golem_worker_service_base::service::component::ComponentService;
use golem_worker_service_base::service::worker::{
    proxy_worker_connection, InvocationParameters, WorkerStream,
};
use payload::Binary;
use poem::web::websocket::{BoxWebSocketUpgraded, WebSocket};
use poem::{Body, Request};
use poem_openapi::param::{Header, Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tap::TapFallible;
use tracing::Instrument;

const WORKER_CONNECT_PING_INTERVAL: Duration = Duration::from_secs(30);
const WORKER_CONNECT_PING_TIMEOUT: Duration = Duration::from_secs(15);

pub struct WorkerApi {
    pub component_service: Arc<dyn ComponentService<DefaultNamespace, EmptyAuthCtx>>,
    pub worker_service: WorkerService,
}

type Result<T> = std::result::Result<T, WorkerApiBaseError>;

#[OpenApi(prefix_path = "/v1/components", tag = ApiTags::Worker)]
impl WorkerApi {
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
        request: Json<WorkerCreationRequest>,
    ) -> Result<Json<WorkerCreationResponse>> {
        let record = recorded_http_api_request!(
            "launch_new_worker",
            component_id = component_id.0.to_string(),
            name = request.name
        );

        let response = self
            .launch_new_worker_internal(component_id.0, request.0)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn launch_new_worker_internal(
        &self,
        component_id: ComponentId,
        request: WorkerCreationRequest,
    ) -> Result<Json<WorkerCreationResponse>> {
        let latest_component = self
            .component_service
            .get_latest(&component_id, &EmptyAuthCtx::default())
            .await
            .tap_err(|error| tracing::error!("Error getting latest component: {:?}", error))
            .map_err(|error| {
                WorkerApiBaseError::NotFound(Json(ErrorBody {
                    error: format!(
                        "Couldn't retrieve the component: {}. error: {}",
                        &component_id, error
                    ),
                }))
            })?;

        let WorkerCreationRequest { name, args, env } = request;

        let worker_id = make_worker_id(component_id, name)?;
        let worker_id = self
            .worker_service
            .create(
                &worker_id,
                latest_component.versioned_component_id.version,
                args,
                env,
                empty_worker_metadata(),
            )
            .await?;
        Ok(Json(WorkerCreationResponse {
            worker_id,
            component_version: latest_component.versioned_component_id.version,
        }))
    }

    /// Delete a worker
    ///
    /// Interrupts and deletes an existing worker.
    #[oai(
        path = "/:component_id/workers/:worker_name",
        method = "delete",
        operation_id = "delete_worker"
    )]
    async fn delete_worker(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
    ) -> Result<Json<DeleteWorkerResponse>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        let record =
            recorded_http_api_request!("delete_worker", worker_id = worker_id.to_string(),);

        let response = self
            .worker_service
            .delete(&worker_id, empty_worker_metadata())
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(DeleteWorkerResponse {}));

        record.result(response)
    }

    /// Invoke a function and await its resolution on a new worker with a random generated name
    ///
    /// Ideal for invoking ephemeral components, but works with durable ones as well.
    /// Supply the parameters in the request body as JSON.
    #[oai(
        path = "/:component_id/invoke-and-await",
        method = "post",
        operation_id = "invoke_and_await_function_without_name"
    )]
    async fn invoke_and_await_function_without_name(
        &self,
        request: &Request,
        component_id: Path<ComponentId>,
        #[oai(name = "Idempotency-Key")] idempotency_key: Header<Option<IdempotencyKey>>,
        function: Query<String>,
        params: Json<InvokeParameters>,
    ) -> Result<Json<InvokeResult>> {
        let worker_id = make_target_worker_id(component_id.0, None)?;

        let record = recorded_http_api_request!(
            "invoke_and_await_function_without_name",
            worker_id = worker_id.to_string(),
            idempotency_key = idempotency_key.0.as_ref().map(|v| v.value.clone()),
            function = function.0
        );

        let response = self
            .invoke_and_await_function_without_name_internal(
                request,
                worker_id,
                idempotency_key.0,
                function.0,
                params.0,
            )
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn invoke_and_await_function_without_name_internal(
        &self,
        request: &Request,
        worker_id: TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        params: InvokeParameters,
    ) -> Result<Json<InvokeResult>> {
        let invocation_context = grpc_invocation_context_from_request(request);
        let params =
            InvocationParameters::from_optionally_type_annotated_value_jsons(params.params)
                .map_err(|errors| WorkerApiBaseError::BadRequest(Json(ErrorsBody { errors })))?;

        let result = match params {
            InvocationParameters::TypedProtoVals(vals) => {
                self.worker_service
                    .validate_and_invoke_and_await_typed(
                        &worker_id,
                        idempotency_key,
                        function,
                        vals,
                        Some(invocation_context),
                        empty_worker_metadata(),
                    )
                    .await?
            }
            InvocationParameters::RawJsonStrings(jsons) => {
                self.worker_service
                    .invoke_and_await_json(
                        &worker_id,
                        idempotency_key,
                        function,
                        jsons,
                        Some(invocation_context),
                        empty_worker_metadata(),
                    )
                    .await?
            }
        };
        Ok(Json(InvokeResult { result }))
    }

    /// Invoke a function and await its resolution
    ///
    /// Supply the parameters in the request body as JSON.
    #[oai(
        path = "/:component_id/workers/:worker_name/invoke-and-await",
        method = "post",
        operation_id = "invoke_and_await_function"
    )]
    async fn invoke_and_await_function(
        &self,
        request: &Request,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        #[oai(name = "Idempotency-Key")] idempotency_key: Header<Option<IdempotencyKey>>,
        function: Query<String>,
        params: Json<InvokeParameters>,
    ) -> Result<Json<InvokeResult>> {
        let worker_id = make_target_worker_id(component_id.0, Some(worker_name.0))?;

        let record = recorded_http_api_request!(
            "invoke_and_await_function",
            worker_id = worker_id.to_string(),
            idempotency_key = idempotency_key.0.as_ref().map(|v| v.value.clone()),
            function = function.0
        );

        let response = self
            .invoke_and_await_function_internal(
                request,
                worker_id,
                idempotency_key.0,
                function.0,
                params.0,
            )
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn invoke_and_await_function_internal(
        &self,
        request: &Request,
        worker_id: TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        params: InvokeParameters,
    ) -> Result<Json<InvokeResult>> {
        let invocation_context = grpc_invocation_context_from_request(request);

        let params =
            InvocationParameters::from_optionally_type_annotated_value_jsons(params.params)
                .map_err(|errors| WorkerApiBaseError::BadRequest(Json(ErrorsBody { errors })))?;

        let result = match params {
            InvocationParameters::TypedProtoVals(vals) => {
                self.worker_service
                    .validate_and_invoke_and_await_typed(
                        &worker_id,
                        idempotency_key,
                        function,
                        vals,
                        Some(invocation_context),
                        empty_worker_metadata(),
                    )
                    .await?
            }
            InvocationParameters::RawJsonStrings(jsons) => {
                self.worker_service
                    .invoke_and_await_json(
                        &worker_id,
                        idempotency_key,
                        function,
                        jsons,
                        Some(invocation_context),
                        empty_worker_metadata(),
                    )
                    .await?
            }
        };

        Ok(Json(InvokeResult { result }))
    }

    /// Invoke a function
    ///
    /// Ideal for invoking ephemeral components, but works with durable ones as well.
    /// Triggers the execution of a function and immediately returns.
    #[oai(
        path = "/:component_id/invoke",
        method = "post",
        operation_id = "invoke_function_without_name"
    )]
    async fn invoke_function_without_name(
        &self,
        request: &Request,
        component_id: Path<ComponentId>,
        #[oai(name = "Idempotency-Key")] idempotency_key: Header<Option<IdempotencyKey>>,
        function: Query<String>,
        params: Json<InvokeParameters>,
    ) -> Result<Json<InvokeResponse>> {
        let worker_id = make_target_worker_id(component_id.0, None)?;

        let record = recorded_http_api_request!(
            "invoke_function_without_name",
            worker_id = worker_id.to_string(),
            idempotency_key = idempotency_key.0.as_ref().map(|v| v.value.clone()),
            function = function.0
        );

        let response = self
            .invoke_function_without_name_internal(
                request,
                worker_id,
                idempotency_key.0,
                function.0,
                params.0,
            )
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn invoke_function_without_name_internal(
        &self,
        request: &Request,
        worker_id: TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        params: InvokeParameters,
    ) -> Result<Json<InvokeResponse>> {
        let invocation_context = grpc_invocation_context_from_request(request);

        let params =
            InvocationParameters::from_optionally_type_annotated_value_jsons(params.params)
                .map_err(|errors| WorkerApiBaseError::BadRequest(Json(ErrorsBody { errors })))?;

        match params {
            InvocationParameters::TypedProtoVals(vals) => {
                self.worker_service
                    .validate_and_invoke(
                        &worker_id,
                        idempotency_key,
                        function,
                        vals,
                        Some(invocation_context),
                        empty_worker_metadata(),
                    )
                    .await?
            }
            InvocationParameters::RawJsonStrings(jsons) => {
                self.worker_service
                    .invoke_json(
                        &worker_id,
                        idempotency_key,
                        function,
                        jsons,
                        Some(invocation_context),
                        empty_worker_metadata(),
                    )
                    .await?
            }
        };

        Ok(Json(InvokeResponse {}))
    }

    /// Invoke a function
    ///
    /// Triggers the execution of a function and immediately returns.
    #[oai(
        path = "/:component_id/workers/:worker_name/invoke",
        method = "post",
        operation_id = "invoke_function"
    )]
    async fn invoke_function(
        &self,
        request: &Request,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        #[oai(name = "Idempotency-Key")] idempotency_key: Header<Option<IdempotencyKey>>,
        function: Query<String>,
        params: Json<InvokeParameters>,
    ) -> Result<Json<InvokeResponse>> {
        let worker_id = make_target_worker_id(component_id.0, Some(worker_name.0))?;

        let record = recorded_http_api_request!(
            "invoke_function",
            worker_id = worker_id.to_string(),
            idempotency_key = idempotency_key.0.as_ref().map(|v| v.value.clone()),
            function = function.0
        );

        let response = self
            .invoke_function_internal(request, worker_id, idempotency_key.0, function.0, params.0)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn invoke_function_internal(
        &self,
        request: &Request,
        worker_id: TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        params: InvokeParameters,
    ) -> Result<Json<InvokeResponse>> {
        let params =
            InvocationParameters::from_optionally_type_annotated_value_jsons(params.params)
                .map_err(|errors| WorkerApiBaseError::BadRequest(Json(ErrorsBody { errors })))?;

        let invocation_context = grpc_invocation_context_from_request(request);

        match params {
            InvocationParameters::TypedProtoVals(vals) => {
                self.worker_service
                    .validate_and_invoke(
                        &worker_id,
                        idempotency_key,
                        function,
                        vals,
                        Some(invocation_context),
                        empty_worker_metadata(),
                    )
                    .await?
            }
            InvocationParameters::RawJsonStrings(jsons) => {
                self.worker_service
                    .invoke_json(
                        &worker_id,
                        idempotency_key,
                        function,
                        jsons,
                        Some(invocation_context),
                        empty_worker_metadata(),
                    )
                    .await?
            }
        }

        Ok(Json(InvokeResponse {}))
    }

    /// Complete a promise
    ///
    /// Completes a promise with a given custom array of bytes.
    /// The promise must be previously created from within the worker, and it's identifier (a combination of a worker identifier and an oplogIdx ) must be sent out to an external caller so it can use this endpoint to mark the promise completed.
    /// The data field is sent back to the worker, and it has no predefined meaning.
    #[oai(
        path = "/:component_id/workers/:worker_name/complete",
        method = "post",
        operation_id = "complete_promise"
    )]
    async fn complete_promise(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        params: Json<CompleteParameters>,
    ) -> Result<Json<bool>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        let record =
            recorded_http_api_request!("complete_promise", worker_id = worker_id.to_string());

        let CompleteParameters { oplog_idx, data } = params.0;

        let response = self
            .worker_service
            .complete_promise(&worker_id, oplog_idx, data, empty_worker_metadata())
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(Json);

        record.result(response)
    }

    /// Interrupt a worker
    ///
    /// Interrupts the execution of a worker.
    /// The worker's status will be Interrupted unless the recover-immediately parameter was used, in which case it remains as it was.
    /// An interrupted worker can be still used, and it is going to be automatically resumed the first time it is used.
    /// For example in case of a new invocation, the previously interrupted invocation is continued before the new one gets processed.
    #[oai(
        path = "/:component_id/workers/:worker_name/interrupt",
        method = "post",
        operation_id = "interrupt_worker"
    )]
    async fn interrupt_worker(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        #[oai(name = "recovery-immediately")] recover_immediately: Query<Option<bool>>,
    ) -> Result<Json<InterruptResponse>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        let record =
            recorded_http_api_request!("interrupt_worker", worker_id = worker_id.to_string());

        let response = self
            .worker_service
            .interrupt(
                &worker_id,
                recover_immediately.0.unwrap_or(false),
                empty_worker_metadata(),
            )
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(InterruptResponse {}));

        record.result(response)
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
        path = "/:component_id/workers/:worker_name",
        method = "get",
        operation_id = "get_worker_metadata"
    )]
    async fn get_worker_metadata(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
    ) -> Result<Json<WorkerMetadata>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        let record =
            recorded_http_api_request!("get_worker_metadata", worker_id = worker_id.to_string());

        let response = self
            .worker_service
            .get_metadata(&worker_id, empty_worker_metadata())
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(Json);

        record.result(response)
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
        filter: Query<Option<Vec<String>>>,
        cursor: Query<Option<String>>,
        count: Query<Option<u64>>,
        precise: Query<Option<bool>>,
    ) -> Result<Json<WorkersMetadataResponse>> {
        let record = recorded_http_api_request!(
            "get_workers_metadata",
            component_id = component_id.0.to_string()
        );

        let response = self
            .get_workers_metadata_internal(component_id.0, filter.0, cursor.0, count.0, precise.0)
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
    ) -> Result<Json<WorkersMetadataResponse>> {
        let filter = match filter {
            Some(filters) if !filters.is_empty() => {
                Some(WorkerFilter::from(filters).map_err(|e| {
                    WorkerApiBaseError::BadRequest(Json(ErrorsBody { errors: vec![e] }))
                })?)
            }
            _ => None,
        };

        let cursor = match cursor {
            Some(cursor) => Some(ScanCursor::from_str(&cursor).map_err(|e| {
                WorkerApiBaseError::BadRequest(Json(ErrorsBody { errors: vec![e] }))
            })?),
            None => None,
        };

        let (cursor, workers) = self
            .worker_service
            .find_metadata(
                &component_id,
                filter,
                cursor.unwrap_or_default(),
                count.unwrap_or(50),
                precise.unwrap_or(false),
                empty_worker_metadata(),
            )
            .await?;

        Ok(Json(WorkersMetadataResponse { workers, cursor }))
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
    ) -> Result<Json<WorkersMetadataResponse>> {
        let record = recorded_http_api_request!(
            "find_workers_metadata",
            component_id = component_id.0.to_string()
        );

        let response = self
            .worker_service
            .find_metadata(
                &component_id.0,
                params.filter.clone(),
                params.cursor.clone().unwrap_or_default(),
                params.count.unwrap_or(50),
                params.precise.unwrap_or(false),
                empty_worker_metadata(),
            )
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|(cursor, workers)| Json(WorkersMetadataResponse { workers, cursor }));

        record.result(response)
    }

    /// Resume a worker
    #[oai(
        path = "/:component_id/workers/:worker_name/resume",
        method = "post",
        operation_id = "resume_worker"
    )]
    async fn resume_worker(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
    ) -> Result<Json<ResumeResponse>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        let record = recorded_http_api_request!("resume_worker", worker_id = worker_id.to_string());
        let response = self
            .worker_service
            .resume(&worker_id, empty_worker_metadata(), false)
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(ResumeResponse {}));

        record.result(response)
    }

    /// Update a worker
    #[oai(
        path = "/:component_id/workers/:worker_name/update",
        method = "post",
        operation_id = "update_worker"
    )]
    async fn update_worker(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        params: Json<UpdateWorkerRequest>,
    ) -> Result<Json<UpdateWorkerResponse>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        let record = recorded_http_api_request!("update_worker", worker_id = worker_id.to_string());

        let response = self
            .worker_service
            .update(
                &worker_id,
                params.mode.clone().into(),
                params.target_version,
                empty_worker_metadata(),
            )
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(UpdateWorkerResponse {}));

        record.result(response)
    }

    /// Get or search the oplog of a worker
    #[oai(
        path = "/:component_id/workers/:worker_name/oplog",
        method = "get",
        operation_id = "get_oplog"
    )]
    async fn get_oplog(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        from: Query<Option<u64>>,
        count: Query<u64>,
        cursor: Query<Option<OplogCursor>>,
        query: Query<Option<String>>,
    ) -> Result<Json<GetOplogResponse>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;
        let record = recorded_http_api_request!("get_oplog", worker_id = worker_id.to_string());

        let response = self
            .get_oplog_internal(worker_id, from.0, count.0, cursor.0, query.0)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn get_oplog_internal(
        &self,
        worker_id: WorkerId,
        from: Option<u64>,
        count: u64,
        cursor: Option<OplogCursor>,
        query: Option<String>,
    ) -> Result<Json<GetOplogResponse>> {
        let response = match (from, query) {
            (Some(_), Some(_)) => Err(WorkerApiBaseError::BadRequest(Json(ErrorsBody {
                errors: vec![
                    "Cannot specify both the 'from' and the 'query' parameters".to_string()
                ],
            })))?,
            (Some(from), None) => {
                self.worker_service
                    .get_oplog(
                        &worker_id,
                        OplogIndex::from_u64(from),
                        cursor,
                        count,
                        empty_worker_metadata(),
                    )
                    .await?
            }
            (None, Some(query)) => {
                self.worker_service
                    .search_oplog(&worker_id, cursor, count, query, empty_worker_metadata())
                    .await?
            }
            (None, None) => {
                self.worker_service
                    .get_oplog(
                        &worker_id,
                        OplogIndex::INITIAL,
                        cursor,
                        count,
                        empty_worker_metadata(),
                    )
                    .await?
            }
        };

        Ok(Json(response))
    }

    /// List files in a worker
    #[oai(
        path = "/:component_id/workers/:worker_name/files/:file_name",
        method = "get",
        operation_id = "get_files"
    )]
    async fn get_file(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        file_name: Path<String>,
    ) -> Result<Json<GetFilesResponse>> {
        let worker_id = make_target_worker_id(component_id.0, Some(worker_name.0))?;
        let path = make_component_file_path(file_name.0)?;
        let record = recorded_http_api_request!("get_file", worker_id = worker_id.to_string());

        let response = self
            .worker_service
            .list_directory(&worker_id, path, empty_worker_metadata())
            .instrument(record.span.clone())
            .await
            .map(|s| {
                Json(GetFilesResponse {
                    nodes: s.into_iter().map(|n| n.into()).collect(),
                })
            })
            .map_err(|e| e.into());

        record.result(response)
    }

    /// Get contents of a file in a worker
    #[oai(
        path = "/:component_id/workers/:worker_name/file-contents/:file_name",
        method = "get",
        operation_id = "get_file_content"
    )]
    async fn get_file_content(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        file_name: Path<String>,
    ) -> Result<Binary<Body>> {
        let worker_id = make_target_worker_id(component_id.0, Some(worker_name.0))?;
        let path = make_component_file_path(file_name.0)?;
        let record = recorded_http_api_request!("get_files", worker_id = worker_id.to_string());

        let response = self
            .worker_service
            .get_file_contents(&worker_id, path, empty_worker_metadata())
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|bytes| {
                Binary(Body::from_bytes_stream(
                    bytes.map_err(|e| std::io::Error::other(e.to_string())),
                ))
            });

        record.result(response)
    }

    /// Activate a plugin
    ///
    /// The plugin must be one of the installed plugins for the worker's current component version.
    #[oai(
        path = "/:component_id/workers/:worker_name/activate-plugin",
        method = "post",
        operation_id = "activate_plugin"
    )]
    async fn activate_plugin(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        #[oai(name = "plugin-installation-id")] plugin_installation_id: Query<PluginInstallationId>,
    ) -> Result<Json<ActivatePluginResponse>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        let record = recorded_http_api_request!(
            "activate_plugin",
            worker_id = worker_id.to_string(),
            plugin_installation_id = plugin_installation_id.to_string()
        );

        let response = self
            .worker_service
            .activate_plugin(
                &worker_id,
                &plugin_installation_id.0,
                empty_worker_metadata(),
            )
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(ActivatePluginResponse {}));

        record.result(response)
    }

    /// Deactivate a plugin
    ///
    /// The plugin must be one of the installed plugins for the worker's current component version.
    #[oai(
        path = "/:component_id/workers/:worker_name/deactivate-plugin",
        method = "post",
        operation_id = "deactivate_plugin"
    )]
    async fn deactivate_plugin(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        #[oai(name = "plugin-installation-id")] plugin_installation_id: Query<PluginInstallationId>,
    ) -> Result<Json<DeactivatePluginResponse>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        let record = recorded_http_api_request!(
            "activate_plugin",
            worker_id = worker_id.to_string(),
            plugin_installation_id = plugin_installation_id.to_string()
        );

        let response = self
            .worker_service
            .deactivate_plugin(
                &worker_id,
                &plugin_installation_id.0,
                empty_worker_metadata(),
            )
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(DeactivatePluginResponse {}));

        record.result(response)
    }

    /// Revert a worker
    ///
    /// Reverts a worker by undoing either the last few invocations or the last few recorded oplog entries.
    #[oai(
        path = "/:component_id/workers/:worker_name/revert",
        method = "post",
        operation_id = "revert_worker"
    )]
    async fn revert_worker(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        target: Json<RevertWorkerTarget>,
    ) -> Result<Json<RevertWorkerResponse>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        let record =
            recorded_http_api_request!("revert_worker", worker_id = worker_id.to_string(),);

        let response = self
            .worker_service
            .revert_worker(&worker_id, target.0, empty_worker_metadata())
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(RevertWorkerResponse {}));

        record.result(response)
    }

    /// Cancels a pending invocation if it has not started yet
    ///
    /// The invocation to be cancelled is identified by the idempotency key passed to the invoke API.
    #[oai(
        path = "/:component_id/workers/:worker_name/invocations/:idempotency_key",
        method = "delete",
        operation_id = "cancel_invocation"
    )]
    async fn cancel_invocation(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        idempotency_key: Path<IdempotencyKey>,
    ) -> Result<Json<CancelInvocationResponse>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        let record = recorded_http_api_request!(
            "cancel_invocation",
            worker_id = worker_id.to_string(),
            idempotency_key = idempotency_key.0.to_string(),
        );

        let response = self
            .worker_service
            .cancel_invocation(&worker_id, &idempotency_key.0, empty_worker_metadata())
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|canceled| Json(CancelInvocationResponse { canceled }));

        record.result(response)
    }

    /// Connect to a worker using a websocket and stream events
    #[oai(
        path = "/:component_id/workers/:worker_name/connect",
        method = "get",
        operation_id = "worker_connect"
    )]
    pub async fn worker_connect(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        websocket: WebSocket,
    ) -> Result<BoxWebSocketUpgraded> {
        let (worker_id, worker_stream) =
            connect_to_worker(&self.worker_service, component_id.0, worker_name.0).await?;

        let upgraded: BoxWebSocketUpgraded = websocket.on_upgrade(Box::new(|socket_stream| {
            Box::pin(async move {
                let (sink, stream) = socket_stream.split();
                let _ = proxy_worker_connection(
                    worker_id,
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
}

fn make_worker_id(
    component_id: ComponentId,
    worker_name: String,
) -> std::result::Result<WorkerId, WorkerApiBaseError> {
    WorkerId::validate_worker_name(&worker_name).map_err(|error| {
        WorkerApiBaseError::BadRequest(Json(ErrorsBody {
            errors: vec![format!("Invalid worker name: {error}")],
        }))
    })?;
    Ok(WorkerId {
        component_id,
        worker_name,
    })
}

fn make_target_worker_id(
    component_id: ComponentId,
    worker_name: Option<String>,
) -> std::result::Result<TargetWorkerId, WorkerApiBaseError> {
    if let Some(worker_name) = &worker_name {
        WorkerId::validate_worker_name(worker_name).map_err(|error| {
            WorkerApiBaseError::BadRequest(Json(ErrorsBody {
                errors: vec![format!("Invalid worker name: {error}")],
            }))
        })?;
    }

    Ok(TargetWorkerId {
        component_id,
        worker_name,
    })
}

fn make_component_file_path(
    name: String,
) -> std::result::Result<ComponentFilePath, WorkerApiBaseError> {
    ComponentFilePath::from_rel_str(&name).map_err(|error| {
        WorkerApiBaseError::BadRequest(Json(ErrorsBody {
            errors: vec![format!("Invalid file name: {error}")],
        }))
    })
}

async fn connect_to_worker(
    worker_service: &WorkerService,
    component_id: ComponentId,
    worker_name: String,
) -> Result<(WorkerId, WorkerStream<LogEvent>)> {
    WorkerId::validate_worker_name(&worker_name).map_err(|e| {
        WorkerApiBaseError::BadRequest(Json(ErrorsBody {
            errors: vec![format!("Invalid worker name: {e}")],
        }))
    })?;
    let worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: worker_name.clone(),
    };

    let record = recorded_http_api_request!("connect_worker", worker_id = worker_id.to_string());

    let result = worker_service
        .connect(&worker_id, empty_worker_metadata())
        .instrument(record.span.clone())
        .await;

    match result {
        Ok(worker_stream) => record.succeed(Ok((worker_id, worker_stream))),
        Err(error) => {
            tracing::error!("Error connecting to worker: {error}");
            let error = WorkerApiBaseError::from(error);
            let error = record.fail(error.clone(), &error);
            Err(error)
        }
    }
}
