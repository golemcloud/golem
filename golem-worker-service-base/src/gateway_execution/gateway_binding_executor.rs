// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::gateway_binding::{
    GatewayRequestDetails, ResolvedBinding, ResolvedGatewayBinding, ResolvedWorkerBinding,
    RibInputTypeMismatch, RibInputValueResolver, StaticBinding,
};
use crate::gateway_execution::file_server_binding_handler::{
    FileServerBindingHandler, FileServerBindingResult,
};
use crate::gateway_execution::to_response::ToResponse;
use crate::gateway_middleware::{Cors as CorsPreflight, Middlewares};
use crate::gateway_rib_interpreter::{EvaluationError, WorkerServiceRibInterpreter};
use async_trait::async_trait;
use rib::{RibInput, RibResult};
use std::fmt::Debug;
use std::sync::Arc;

#[async_trait]
pub trait GatewayBindingExecutor<Namespace, Response> {
    async fn execute_binding(&self, binding: &ResolvedGatewayBinding<Namespace>) -> Response
    where
        RibResult: ToResponse<Response>,
        EvaluationError: ToResponse<Response>,
        RibInputTypeMismatch: ToResponse<Response>,
        FileServerBindingResult: ToResponse<Response>,
        CorsPreflight: ToResponse<Response>;
}

pub struct DefaultGatewayBindingExecutor<Namespace> {
    pub evaluator: Arc<dyn WorkerServiceRibInterpreter<Namespace> + Sync + Send>,
    pub file_server_binding_handler: Arc<dyn FileServerBindingHandler<Namespace> + Sync + Send>,
}

impl<Namespace: Clone> DefaultGatewayBindingExecutor<Namespace> {
    pub fn new(
        evaluator: Arc<dyn WorkerServiceRibInterpreter<Namespace> + Sync + Send>,
        file_server_binding_handler: Arc<dyn FileServerBindingHandler<Namespace> + Sync + Send>,
    ) -> Self {
        Self {
            evaluator,
            file_server_binding_handler,
        }
    }

    async fn resolve_rib_inputs<R>(
        &self,
        request_details: &GatewayRequestDetails,
        resolved_worker_binding: &ResolvedWorkerBinding<Namespace>,
    ) -> Result<(RibInput, RibInput), R>
    where
        RibInputTypeMismatch: ToResponse<R>,
    {
        let request_rib_input = request_details
            .resolve_rib_input_value(&resolved_worker_binding.compiled_response_mapping.rib_input)
            .map_err(|err| {
                err.to_response(request_details, &resolved_worker_binding.middlewares)
            })?;

        let worker_rib_input = resolved_worker_binding
            .worker_detail
            .resolve_rib_input_value(&resolved_worker_binding.compiled_response_mapping.rib_input)
            .map_err(|err| {
                err.to_response(request_details, &resolved_worker_binding.middlewares)
            })?;

        Ok((request_rib_input, worker_rib_input))
    }

    async fn get_rib_result(
        &self,
        request_rib_input: RibInput,
        worker_rib_input: RibInput,
        resolved_worker_binding: &ResolvedWorkerBinding<Namespace>,
    ) -> Result<RibResult, EvaluationError> {
        let rib_input = request_rib_input.merge(worker_rib_input);
        self.evaluator
            .evaluate(
                resolved_worker_binding.worker_detail.worker_name.as_deref(),
                &resolved_worker_binding
                    .worker_detail
                    .component_id
                    .component_id,
                &resolved_worker_binding.worker_detail.idempotency_key,
                &resolved_worker_binding
                    .compiled_response_mapping
                    .response_mapping_compiled,
                &rib_input,
                resolved_worker_binding.namespace.clone(),
            )
            .await
    }

    async fn handle_worker_binding<R>(
        &self,
        binding: &ResolvedGatewayBinding<Namespace>,
        resolved_binding: &ResolvedWorkerBinding<Namespace>,
    ) -> R
    where
        RibResult: ToResponse<R>,
        EvaluationError: ToResponse<R>,
        RibInputTypeMismatch: ToResponse<R>,
    {
        match self
            .resolve_rib_inputs(&binding.request_details, resolved_binding)
            .await
        {
            Ok((request_rib_input, worker_rib_input)) => {
                match self
                    .get_rib_result(request_rib_input, worker_rib_input, resolved_binding)
                    .await
                {
                    Ok(result) => {
                        result.to_response(&binding.request_details, &resolved_binding.middlewares)
                    }
                    Err(err) => {
                        err.to_response(&binding.request_details, &resolved_binding.middlewares)
                    }
                }
            }
            Err(err_response) => err_response,
        }
    }

    async fn handle_file_server_binding<R>(
        &self,
        binding: &ResolvedGatewayBinding<Namespace>,
        resolved_binding: &ResolvedWorkerBinding<Namespace>,
    ) -> R
    where
        FileServerBindingResult: ToResponse<R>,
        RibResult: ToResponse<R>,
        EvaluationError: ToResponse<R>,
        RibInputTypeMismatch: ToResponse<R>,
    {
        match self
            .resolve_rib_inputs(&binding.request_details, resolved_binding)
            .await
        {
            Ok((request_rib_input, worker_rib_input)) => {
                match self
                    .get_rib_result(request_rib_input, worker_rib_input, resolved_binding)
                    .await
                {
                    Ok(worker_response) => self
                        .file_server_binding_handler
                        .handle_file_server_binding_result(
                            &resolved_binding.namespace,
                            &resolved_binding.worker_detail,
                            worker_response,
                        )
                        .await
                        .to_response(&binding.request_details, &resolved_binding.middlewares),
                    Err(err) => {
                        err.to_response(&binding.request_details, &resolved_binding.middlewares)
                    }
                }
            }
            Err(err_response) => err_response,
        }
    }
}

#[async_trait]
impl<Namespace: Clone + Send + Sync, Request: Debug + Send + Sync>
    GatewayBindingExecutor<Namespace, Request> for DefaultGatewayBindingExecutor<Namespace>
{
    async fn execute_binding(&self, binding: &ResolvedGatewayBinding<Namespace>) -> Request
    where
        RibResult: ToResponse<Request>,
        EvaluationError: ToResponse<Request>,
        RibInputTypeMismatch: ToResponse<Request>,
        FileServerBindingResult: ToResponse<Request>,
        CorsPreflight: ToResponse<Request>,
    {
        match &binding.resolved_binding {
            ResolvedBinding::Worker(resolved_binding) => {
                self.handle_worker_binding::<Request>(binding, resolved_binding)
                    .await
            }
            ResolvedBinding::FileServer(resolved_binding) => {
                self.handle_file_server_binding::<Request>(binding, resolved_binding)
                    .await
            }
            ResolvedBinding::Static(StaticBinding::HttpCorsPreflight(cors_preflight)) => {
                cors_preflight
                    .clone()
                    .to_response(&binding.request_details, &Middlewares::default())
            }
        }
    }
}
