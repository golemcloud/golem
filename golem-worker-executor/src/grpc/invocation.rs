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

use crate::services::HasComponentService;
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use golem_common::base_model::WorkerId;
use golem_common::model::component::{ComponentDto, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::{IdempotencyKey, WorkerMetadata};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_wasm::analysis::{AnalysedExport, AnalysedFunction, AnalysedFunctionParameter};
use golem_wasm::json::ValueAndTypeJsonExtensions;
use golem_wasm::protobuf::Val;
use golem_wasm::{Value, ValueAndType};
use rib::{ParsedFunctionName, ParsedFunctionSite};
use std::collections::BTreeMap;
use std::sync::Arc;
use tracing::warn;

pub trait CanStartWorker {
    fn environment_id(&self) -> Result<EnvironmentId, WorkerExecutorError>;
    fn worker_id(&self) -> Result<WorkerId, WorkerExecutorError>;
    fn env(&self) -> Option<Vec<(String, String)>>;
    fn wasi_config_vars(&self) -> Result<Option<BTreeMap<String, String>>, WorkerExecutorError>;
    fn parent(&self) -> Option<WorkerId>;
    fn maybe_invocation_context(&self) -> Option<InvocationContextStack> {
        None
    }
    fn auth_ctx(&self) -> Result<AuthCtx, WorkerExecutorError>;
}

pub trait GrpcInvokeRequest: CanStartWorker {
    async fn input<Ctx: WorkerCtx>(
        &self,
        worker: &Arc<Worker<Ctx>>,
    ) -> Result<Vec<Val>, WorkerExecutorError>;
    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, WorkerExecutorError>;
    fn name(&self) -> String;
    fn invocation_context(&self) -> InvocationContextStack;
}

trait ProtobufInvocationDetails {
    fn proto_worker_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::WorkerId>;
    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId>;
    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext>;
    fn proto_auth_ctx(&self) -> &Option<golem_api_grpc::proto::golem::auth::AuthCtx>;
}

impl<T: ProtobufInvocationDetails> CanStartWorker for T {
    fn environment_id(&self) -> Result<EnvironmentId, WorkerExecutorError> {
        (*self.proto_environment_id())
            .ok_or(WorkerExecutorError::invalid_request(
                "environment_id not found",
            ))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)
    }

    fn worker_id(&self) -> Result<WorkerId, WorkerExecutorError> {
        self.proto_worker_id()
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("worker_id not found"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)
    }

    fn env(&self) -> Option<Vec<(String, String)>> {
        self.proto_invocation_context()
            .as_ref()
            .map(|ctx| ctx.env.clone().into_iter().collect::<Vec<_>>())
    }

    fn wasi_config_vars(&self) -> Result<Option<BTreeMap<String, String>>, WorkerExecutorError> {
        match self.proto_invocation_context() {
            Some(ctx) => Ok(Some(
                ctx.wasi_config_vars
                    .clone()
                    .ok_or(WorkerExecutorError::invalid_request(
                        "wasi_config_vars not found",
                    ))?
                    .into(),
            )),
            None => Ok(None),
        }
    }

    fn parent(&self) -> Option<WorkerId> {
        self.proto_invocation_context().as_ref().and_then(|ctx| {
            ctx.parent
                .as_ref()
                .and_then(|worker_id| worker_id.clone().try_into().ok())
        })
    }

    fn auth_ctx(&self) -> Result<AuthCtx, WorkerExecutorError> {
        self.proto_auth_ctx()
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::GetFileSystemNodeRequest
{
    fn proto_worker_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::WorkerId> {
        &self.worker_id
    }

    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId> {
        &self.environment_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &None
    }

    fn proto_auth_ctx(&self) -> &Option<golem_api_grpc::proto::golem::auth::AuthCtx> {
        &self.auth_ctx
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::GetFileContentsRequest
{
    fn proto_worker_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::WorkerId> {
        &self.worker_id
    }

    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId> {
        &self.environment_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &None
    }

    fn proto_auth_ctx(&self) -> &Option<golem_api_grpc::proto::golem::auth::AuthCtx> {
        &self.auth_ctx
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::InvokeWorkerRequest
{
    fn proto_worker_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::WorkerId> {
        &self.worker_id
    }

    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId> {
        &self.environment_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &self.context
    }

    fn proto_auth_ctx(&self) -> &Option<golem_api_grpc::proto::golem::auth::AuthCtx> {
        &self.auth_ctx
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::InvokeAndAwaitWorkerRequest
{
    fn proto_worker_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::WorkerId> {
        &self.worker_id
    }

    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId> {
        &self.environment_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &self.context
    }

    fn proto_auth_ctx(&self) -> &Option<golem_api_grpc::proto::golem::auth::AuthCtx> {
        &self.auth_ctx
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::InvokeAndAwaitWorkerJsonRequest
{
    fn proto_worker_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::WorkerId> {
        &self.worker_id
    }

    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId> {
        &self.environment_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &self.context
    }

    fn proto_auth_ctx(&self) -> &Option<golem_api_grpc::proto::golem::auth::AuthCtx> {
        &self.auth_ctx
    }
}

impl GrpcInvokeRequest for golem_api_grpc::proto::golem::workerexecutor::v1::InvokeWorkerRequest {
    async fn input<Ctx: WorkerCtx>(
        &self,
        _worker: &Arc<Worker<Ctx>>,
    ) -> Result<Vec<Val>, WorkerExecutorError> {
        Ok(self.input.clone())
    }

    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, WorkerExecutorError> {
        Ok(self.idempotency_key.clone().map(IdempotencyKey::from))
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn invocation_context(&self) -> InvocationContextStack {
        from_proto_invocation_context(&self.context)
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::InvokeJsonWorkerRequest
{
    fn proto_worker_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::WorkerId> {
        &self.worker_id
    }

    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId> {
        &self.environment_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &self.context
    }

    fn proto_auth_ctx(&self) -> &Option<golem_api_grpc::proto::golem::auth::AuthCtx> {
        &self.auth_ctx
    }
}

impl GrpcInvokeRequest
    for golem_api_grpc::proto::golem::workerexecutor::v1::InvokeAndAwaitWorkerRequest
{
    async fn input<Ctx: WorkerCtx>(
        &self,
        _worker: &Arc<Worker<Ctx>>,
    ) -> Result<Vec<Val>, WorkerExecutorError> {
        Ok(self.input.clone())
    }

    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, WorkerExecutorError> {
        Ok(self.idempotency_key.clone().map(IdempotencyKey::from))
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn invocation_context(&self) -> InvocationContextStack {
        from_proto_invocation_context(&self.context)
    }
}

impl GrpcInvokeRequest
    for golem_api_grpc::proto::golem::workerexecutor::v1::InvokeAndAwaitWorkerJsonRequest
{
    async fn input<Ctx: WorkerCtx>(
        &self,
        worker: &Arc<Worker<Ctx>>,
    ) -> Result<Vec<Val>, WorkerExecutorError> {
        interpret_json_input(&self.name, &self.input, worker).await
    }

    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, WorkerExecutorError> {
        Ok(self.idempotency_key.clone().map(IdempotencyKey::from))
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn invocation_context(&self) -> InvocationContextStack {
        from_proto_invocation_context(&self.context)
    }
}

impl GrpcInvokeRequest
    for golem_api_grpc::proto::golem::workerexecutor::v1::InvokeJsonWorkerRequest
{
    async fn input<Ctx: WorkerCtx>(
        &self,
        worker: &Arc<Worker<Ctx>>,
    ) -> Result<Vec<Val>, WorkerExecutorError> {
        interpret_json_input(&self.name, &self.input, worker).await
    }

    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, WorkerExecutorError> {
        Ok(self.idempotency_key.clone().map(IdempotencyKey::from))
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn invocation_context(&self) -> InvocationContextStack {
        from_proto_invocation_context(&self.context)
    }
}

/// Assumes what component version a worker will execute the next enqueued invocation with
fn assume_future_component_version(metadata: &WorkerMetadata) -> ComponentRevision {
    let mut version = metadata.last_known_status.component_revision;
    for pending_update in &metadata.last_known_status.pending_updates {
        // Assuming this update will succeed
        version = *pending_update.description.target_revision();
    }
    version
}

fn resolve_function<'t>(
    component: &'t ComponentDto,
    function: &str,
) -> Result<(&'t AnalysedFunction, ParsedFunctionName), WorkerExecutorError> {
    let parsed =
        ParsedFunctionName::parse(function).map_err(WorkerExecutorError::invalid_request)?;
    let mut functions = Vec::new();

    for export in component.metadata.exports() {
        match export {
            AnalysedExport::Instance(interface) => {
                if matches!(parsed.site().interface_name(), Some(name) if name == interface.name) {
                    for function in &interface.functions {
                        if parsed.function().function_name() == function.name {
                            functions.push(function);
                        }
                    }
                }
            }
            AnalysedExport::Function(ref f @ AnalysedFunction { name, .. }) => {
                if parsed.site() == &ParsedFunctionSite::Global
                    && parsed.function().function_name() == *name
                {
                    functions.push(f);
                }
            }
        }
    }

    if functions.len() > 1 {
        Err(WorkerExecutorError::invalid_request(format!(
            "Found multiple exported functions with the same name ({function})"
        )))
    } else if let Some(func) = functions.first() {
        Ok((func, parsed))
    } else {
        Err(WorkerExecutorError::invalid_request(format!(
            "Can't find exported function in component ({function})"
        )))
    }
}

async fn interpret_json_input<Ctx: WorkerCtx>(
    function_name: &str,
    input_json_strings: &[String],
    worker: &Arc<Worker<Ctx>>,
) -> Result<Vec<Val>, WorkerExecutorError> {
    let metadata = worker.get_latest_worker_metadata().await;
    let assumed_component_version = assume_future_component_version(&metadata);
    let component_metadata = worker
        .component_service()
        .get_metadata(
            &metadata.worker_id.component_id,
            Some(assumed_component_version),
        )
        .await?;
    let (function, _parsed) = resolve_function(&component_metadata, function_name)?;

    let expected_params: Vec<&AnalysedFunctionParameter> = function.parameters.iter().collect();

    let mut input = Vec::new();
    for (json_string, param) in input_json_strings.iter().zip(expected_params) {
        let json = serde_json::from_str(json_string).map_err(|err| {
            WorkerExecutorError::invalid_request(format!(
                "Invalid JSON parameter for {}: {err}",
                param.name
            ))
        })?;
        let value_and_type =
            ValueAndType::parse_with_type(&json, &param.typ).map_err(|errors| {
                WorkerExecutorError::invalid_request(format!(
                    "Parameter {} has unexpected type: {}",
                    param.name,
                    errors.join(", ")
                ))
            })?;
        let val: Value = value_and_type.value;
        input.push(val.into());
    }

    Ok(input)
}

fn from_proto_invocation_context(
    context: &Option<golem_api_grpc::proto::golem::worker::InvocationContext>,
) -> InvocationContextStack {
    let provided_context = context.as_ref().and_then(|context| {
        context.tracing.as_ref().and_then(|tracing_context| {
            let result: Result<InvocationContextStack, String> = tracing_context.clone().try_into();
            if let Err(err) = &result {
                warn!("Failed to parse tracing context: {}", err);
            }
            result.ok()
        })
    });
    provided_context.unwrap_or_else(InvocationContextStack::fresh)
}
