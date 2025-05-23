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

use crate::error::GolemError;
use crate::services::component::ComponentMetadata;
use crate::services::HasComponentService;
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use crate::GolemTypes;
use golem_api_grpc::proto::golem::common::ResourceLimits as GrpcResourceLimits;
use golem_common::base_model::{TargetWorkerId, WorkerId};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::{AccountId, ComponentVersion, IdempotencyKey, WorkerMetadata};
use golem_wasm_ast::analysis::{AnalysedExport, AnalysedFunction, AnalysedFunctionParameter};
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::Val;
use golem_wasm_rpc::Value;
use rib::{ParsedFunctionName, ParsedFunctionSite};
use std::sync::Arc;
use tracing::warn;

pub trait CanStartWorker {
    fn account_id(&self) -> Result<AccountId, GolemError>;
    fn account_limits(&self) -> Option<GrpcResourceLimits>;
    fn worker_id(&self) -> Result<TargetWorkerId, GolemError>;
    fn args(&self) -> Option<Vec<String>>;
    fn env(&self) -> Option<Vec<(String, String)>>;
    fn parent(&self) -> Option<WorkerId>;
}

pub trait GrpcInvokeRequest: CanStartWorker {
    async fn input<Ctx: WorkerCtx>(
        &self,
        worker: &Arc<Worker<Ctx>>,
    ) -> Result<Vec<Val>, GolemError>;
    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, GolemError>;
    fn name(&self) -> String;
    fn invocation_context(&self) -> InvocationContextStack;
}

trait ProtobufInvocationDetails {
    fn proto_account_id(&self) -> &Option<golem_api_grpc::proto::golem::common::AccountId>;
    fn proto_account_limits(&self)
        -> &Option<golem_api_grpc::proto::golem::common::ResourceLimits>;
    fn proto_worker_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::TargetWorkerId>;
    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext>;
}

impl<T: ProtobufInvocationDetails> CanStartWorker for T {
    fn account_id(&self) -> Result<AccountId, GolemError> {
        Ok(self
            .proto_account_id()
            .clone()
            .ok_or(GolemError::invalid_request("account_id not found"))?
            .into())
    }

    fn account_limits(&self) -> Option<GrpcResourceLimits> {
        *self.proto_account_limits()
    }

    fn worker_id(&self) -> Result<TargetWorkerId, GolemError> {
        self.proto_worker_id()
            .clone()
            .ok_or(GolemError::invalid_request("worker_id not found"))?
            .try_into()
            .map_err(GolemError::invalid_request)
    }

    fn args(&self) -> Option<Vec<String>> {
        self.proto_invocation_context()
            .as_ref()
            .map(|ctx| ctx.args.clone())
    }

    fn env(&self) -> Option<Vec<(String, String)>> {
        self.proto_invocation_context()
            .as_ref()
            .map(|ctx| ctx.env.clone().into_iter().collect::<Vec<_>>())
    }

    fn parent(&self) -> Option<WorkerId> {
        self.proto_invocation_context().as_ref().and_then(|ctx| {
            ctx.parent
                .as_ref()
                .and_then(|worker_id| worker_id.clone().try_into().ok())
        })
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::ListDirectoryRequest
{
    fn proto_account_id(&self) -> &Option<golem_api_grpc::proto::golem::common::AccountId> {
        &self.account_id
    }

    fn proto_account_limits(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::common::ResourceLimits> {
        &self.account_limits
    }

    fn proto_worker_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::TargetWorkerId> {
        &self.worker_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &None
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::GetFileContentsRequest
{
    fn proto_account_id(&self) -> &Option<golem_api_grpc::proto::golem::common::AccountId> {
        &self.account_id
    }

    fn proto_account_limits(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::common::ResourceLimits> {
        &self.account_limits
    }

    fn proto_worker_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::TargetWorkerId> {
        &self.worker_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &None
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::InvokeWorkerRequest
{
    fn proto_account_id(&self) -> &Option<golem_api_grpc::proto::golem::common::AccountId> {
        &self.account_id
    }

    fn proto_account_limits(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::common::ResourceLimits> {
        &self.account_limits
    }

    fn proto_worker_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::TargetWorkerId> {
        &self.worker_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &self.context
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::InvokeAndAwaitWorkerRequest
{
    fn proto_account_id(&self) -> &Option<golem_api_grpc::proto::golem::common::AccountId> {
        &self.account_id
    }

    fn proto_account_limits(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::common::ResourceLimits> {
        &self.account_limits
    }

    fn proto_worker_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::TargetWorkerId> {
        &self.worker_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &self.context
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::InvokeAndAwaitWorkerJsonRequest
{
    fn proto_account_id(&self) -> &Option<golem_api_grpc::proto::golem::common::AccountId> {
        &self.account_id
    }

    fn proto_account_limits(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::common::ResourceLimits> {
        &self.account_limits
    }

    fn proto_worker_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::TargetWorkerId> {
        &self.worker_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &self.context
    }
}

impl GrpcInvokeRequest for golem_api_grpc::proto::golem::workerexecutor::v1::InvokeWorkerRequest {
    async fn input<Ctx: WorkerCtx>(
        &self,
        _worker: &Arc<Worker<Ctx>>,
    ) -> Result<Vec<Val>, GolemError> {
        Ok(self.input.clone())
    }

    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, GolemError> {
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
    fn proto_account_id(&self) -> &Option<golem_api_grpc::proto::golem::common::AccountId> {
        &self.account_id
    }

    fn proto_account_limits(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::common::ResourceLimits> {
        &self.account_limits
    }

    fn proto_worker_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::TargetWorkerId> {
        &self.worker_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &self.context
    }
}

impl GrpcInvokeRequest
    for golem_api_grpc::proto::golem::workerexecutor::v1::InvokeAndAwaitWorkerRequest
{
    async fn input<Ctx: WorkerCtx>(
        &self,
        _worker: &Arc<Worker<Ctx>>,
    ) -> Result<Vec<Val>, GolemError> {
        Ok(self.input.clone())
    }

    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, GolemError> {
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
    ) -> Result<Vec<Val>, GolemError> {
        interpret_json_input(&self.name, &self.input, worker).await
    }

    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, GolemError> {
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
    ) -> Result<Vec<Val>, GolemError> {
        interpret_json_input(&self.name, &self.input, worker).await
    }

    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, GolemError> {
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
fn assume_future_component_version(metadata: &WorkerMetadata) -> ComponentVersion {
    let mut version = metadata.last_known_status.component_version;
    for pending_update in &metadata.last_known_status.pending_updates {
        // Assuming this update will succeed
        version = *pending_update.description.target_version();
    }
    version
}

fn resolve_function<'t, T: GolemTypes>(
    component: &'t ComponentMetadata<T>,
    function: &str,
) -> Result<(&'t AnalysedFunction, ParsedFunctionName), GolemError> {
    let parsed = ParsedFunctionName::parse(function).map_err(GolemError::invalid_request)?;
    let mut functions = Vec::new();

    for export in &component.exports {
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
                    && &parsed.function().function_name() == name
                {
                    functions.push(f);
                }
            }
        }
    }

    if functions.len() > 1 {
        Err(GolemError::invalid_request(format!(
            "Found multiple exported functions with the same name ({function})"
        )))
    } else if let Some(func) = functions.first() {
        Ok((func, parsed))
    } else {
        Err(GolemError::invalid_request(format!(
            "Can't find exported function in component ({function})"
        )))
    }
}

async fn interpret_json_input<Ctx: WorkerCtx>(
    function_name: &str,
    input_json_strings: &[String],
    worker: &Arc<Worker<Ctx>>,
) -> Result<Vec<Val>, GolemError> {
    let metadata = worker.get_metadata()?;
    let assumed_component_version = assume_future_component_version(&metadata);
    let component_metadata = worker
        .component_service()
        .get_metadata(
            &metadata.account_id,
            &metadata.worker_id.component_id,
            Some(assumed_component_version),
        )
        .await?;
    let (function, parsed) = resolve_function::<Ctx::Types>(&component_metadata, function_name)?;

    let expected_params: Vec<&AnalysedFunctionParameter> =
        if parsed.function().is_indexed_resource() {
            function.parameters.iter().skip(1).collect()
        } else {
            function.parameters.iter().collect()
        };

    let mut input = Vec::new();
    for (json_string, param) in input_json_strings.iter().zip(expected_params) {
        let json = serde_json::from_str(json_string).map_err(|err| {
            GolemError::invalid_request(format!("Invalid JSON parameter for {}: {err}", param.name))
        })?;
        let type_annotated_value =
            TypeAnnotatedValue::parse_with_type(&json, &param.typ).map_err(|errors| {
                GolemError::invalid_request(format!(
                    "Parameter {} has unexpected type: {}",
                    param.name,
                    errors.join(", ")
                ))
            })?;
        let val: Value = type_annotated_value.try_into().map_err(|err| {
            GolemError::invalid_request(format!(
                "Invalid parameter value for {}: {err}",
                param.name
            ))
        })?;
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
