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

use golem_common::base_model::agent::Principal;
use golem_common::base_model::WorkerId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use std::collections::BTreeMap;
use tracing::warn;

pub trait CanStartWorker {
    fn environment_id(&self) -> Result<EnvironmentId, WorkerExecutorError>;
    fn worker_id(&self) -> Result<WorkerId, WorkerExecutorError>;
    fn env(&self) -> Option<Vec<(String, String)>>;
    fn config_vars(&self) -> Result<Option<BTreeMap<String, String>>, WorkerExecutorError>;
    fn parent(&self) -> Option<WorkerId>;
    fn maybe_invocation_context(&self) -> Option<InvocationContextStack> {
        None
    }
    fn auth_ctx(&self) -> Result<AuthCtx, WorkerExecutorError>;
    fn principal(&self) -> Principal {
        Principal::anonymous()
    }
}

trait ProtobufInvocationDetails {
    fn proto_worker_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::WorkerId>;
    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId>;
    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext>;
    fn proto_auth_ctx(&self) -> &Option<golem_api_grpc::proto::golem::auth::AuthCtx>;
    fn proto_principal(&self) -> &Option<golem_api_grpc::proto::golem::component::Principal> {
        &None
    }
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

    fn config_vars(&self) -> Result<Option<BTreeMap<String, String>>, WorkerExecutorError> {
        match self.proto_invocation_context() {
            Some(ctx) => Ok(Some(ctx.config_vars.clone().into_iter().collect())),
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

    fn principal(&self) -> Principal {
        self.proto_principal()
            .as_ref()
            .and_then(|p| {
                let result: Result<Principal, String> = p.clone().try_into();
                result.ok()
            })
            .unwrap_or_else(Principal::anonymous)
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

    fn proto_principal(&self) -> &Option<golem_api_grpc::proto::golem::component::Principal> {
        &self.principal
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

    fn proto_principal(&self) -> &Option<golem_api_grpc::proto::golem::component::Principal> {
        &self.principal
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::InvokeAgentRequest
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

    fn proto_principal(&self) -> &Option<golem_api_grpc::proto::golem::component::Principal> {
        &self.principal
    }
}

pub fn from_proto_invocation_context(
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
