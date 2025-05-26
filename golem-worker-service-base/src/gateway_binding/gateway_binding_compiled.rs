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

use crate::gateway_binding::{
    GatewayBinding, IdempotencyKeyCompiled, ResponseMappingCompiled, WorkerBinding,
    WorkerBindingCompiled, WorkerNameCompiled,
};
use crate::gateway_binding::{InvocationContextCompiled, StaticBinding};
use golem_api_grpc::proto::golem::apidefinition::GatewayBindingType as ProtoGatewayBindingType;
use golem_common::model::GatewayBindingType;
use rib::RibOutputTypeInfo;

use super::http_handler_binding::HttpHandlerBindingCompiled;
use super::HttpHandlerBinding;

// A compiled binding is a binding with all existence of Rib Expr
// get replaced with their compiled form - RibByteCode.
#[derive(Debug, Clone, PartialEq)]
pub enum GatewayBindingCompiled {
    Worker(WorkerBindingCompiled),
    Static(StaticBinding),
    FileServer(WorkerBindingCompiled),
    HttpHandler(HttpHandlerBindingCompiled),
}

impl GatewayBindingCompiled {
    pub fn is_static_auth_call_back_binding(&self) -> bool {
        match self {
            GatewayBindingCompiled::Worker(_) => false,
            GatewayBindingCompiled::FileServer(_) => false,
            GatewayBindingCompiled::HttpHandler(_) => false,
            GatewayBindingCompiled::Static(static_binding) => match static_binding {
                StaticBinding::HttpCorsPreflight(_) => false,
                StaticBinding::HttpAuthCallBack(_) => true,
            },
        }
    }
}

impl From<GatewayBindingCompiled> for GatewayBinding {
    fn from(value: GatewayBindingCompiled) -> Self {
        match value {
            GatewayBindingCompiled::Static(static_binding) => {
                GatewayBinding::Static(static_binding)
            }
            GatewayBindingCompiled::Worker(value) => {
                let worker_binding = value.clone();

                let worker_binding = WorkerBinding::from(worker_binding);

                GatewayBinding::Default(worker_binding)
            }
            GatewayBindingCompiled::FileServer(value) => {
                let worker_binding = value.clone();

                let worker_binding = WorkerBinding::from(worker_binding);

                GatewayBinding::FileServer(worker_binding)
            }
            GatewayBindingCompiled::HttpHandler(value) => {
                let http_handler_binding = value.clone();

                let worker_binding = HttpHandlerBinding::from(http_handler_binding);

                GatewayBinding::HttpHandler(worker_binding)
            }
        }
    }
}

impl TryFrom<GatewayBindingCompiled>
    for golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding
{
    type Error = String;
    fn try_from(value: GatewayBindingCompiled) -> Result<Self, String> {
        match value {
            GatewayBindingCompiled::Worker(worker_binding) => {
                Ok(internal::worker_binding_to_gateway_binding_compiled_proto(
                    worker_binding,
                    GatewayBindingType::Default,
                )?)
            }

            GatewayBindingCompiled::FileServer(worker_binding) => {
                Ok(internal::worker_binding_to_gateway_binding_compiled_proto(
                    worker_binding,
                    GatewayBindingType::FileServer,
                )?)
            }

            GatewayBindingCompiled::HttpHandler(http_handler_binding) => {
                Ok(internal::http_handler_to_gateway_binding_compiled_proto(
                    http_handler_binding,
                    GatewayBindingType::HttpHandler,
                )?)
            }

            GatewayBindingCompiled::Static(static_binding) => {
                let binding_type = match static_binding {
                    StaticBinding::HttpCorsPreflight(_) => golem_api_grpc::proto::golem::apidefinition::GatewayBindingType::CorsPreflight,
                    StaticBinding::HttpAuthCallBack(_) => golem_api_grpc::proto::golem::apidefinition::GatewayBindingType::AuthCallBack,
                };

                Ok(
                    golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding {
                        component: None,
                        worker_name: None,
                        compiled_worker_name_expr: None,
                        worker_name_rib_input: None,
                        idempotency_key: None,
                        compiled_idempotency_key_expr: None,
                        idempotency_key_rib_input: None,
                        response: None,
                        compiled_response_expr: None,
                        response_rib_input: None,
                        worker_functions_in_response: None,
                        binding_type: Some(binding_type as i32),
                        static_binding: Some(
                            golem_api_grpc::proto::golem::apidefinition::StaticBinding::try_from(
                                static_binding,
                            )?,
                        ),
                        response_rib_output: None,
                        invocation_context: None,
                        compiled_invocation_context_expr: None,
                        invocation_context_rib_input: None,
                    },
                )
            }
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding>
    for GatewayBindingCompiled
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding,
    ) -> Result<Self, Self::Error> {
        let binding_type = value
            .binding_type
            .unwrap_or(ProtoGatewayBindingType::Default.into());

        let binding_type = ProtoGatewayBindingType::try_from(binding_type)
            .map_err(|e| format!("Failed to convert binding type: {}", e))?;

        match binding_type {
            ProtoGatewayBindingType::FileServer | ProtoGatewayBindingType::Default => {
                // Convert fields for the Worker variant
                let component_id = value
                    .component
                    .ok_or("Missing component_id for Worker")?
                    .try_into()?;

                let worker_name_compiled = match (
                    value.worker_name,
                    value.compiled_worker_name_expr,
                    value.worker_name_rib_input,
                ) {
                    (Some(worker_name), Some(compiled_worker_name), Some(rib_input_type_info)) => {
                        Some(WorkerNameCompiled {
                            worker_name: rib::Expr::try_from(worker_name)?,
                            compiled_worker_name: rib::RibByteCode::try_from(compiled_worker_name)?,
                            rib_input_type_info: rib::RibInputTypeInfo::try_from(
                                rib_input_type_info,
                            )?,
                        })
                    }
                    _ => None,
                };

                let idempotency_key_compiled = match (
                    value.idempotency_key,
                    value.compiled_idempotency_key_expr,
                    value.idempotency_key_rib_input,
                ) {
                    (Some(idempotency_key), Some(compiled_idempotency_key), Some(rib_input)) => {
                        Some(IdempotencyKeyCompiled {
                            idempotency_key: rib::Expr::try_from(idempotency_key)?,
                            compiled_idempotency_key: rib::RibByteCode::try_from(
                                compiled_idempotency_key,
                            )?,
                            rib_input: rib::RibInputTypeInfo::try_from(rib_input)?,
                        })
                    }
                    _ => None,
                };

                let invocation_context_compiled = match (
                    value.invocation_context,
                    value.compiled_invocation_context_expr,
                    value.invocation_context_rib_input,
                ) {
                    (
                        Some(invocation_context),
                        Some(compiled_invocation_context),
                        Some(rib_input),
                    ) => Some(InvocationContextCompiled {
                        invocation_context: rib::Expr::try_from(invocation_context)?,
                        compiled_invocation_context: rib::RibByteCode::try_from(
                            compiled_invocation_context,
                        )?,
                        rib_input: rib::RibInputTypeInfo::try_from(rib_input)?,
                    }),
                    _ => None,
                };

                let response_compiled = ResponseMappingCompiled {
                    response_mapping_expr: rib::Expr::try_from(
                        value.response.ok_or("Missing response for Worker")?,
                    )?,
                    response_mapping_compiled: rib::RibByteCode::try_from(
                        value
                            .compiled_response_expr
                            .ok_or("Missing compiled_response for Worker")?,
                    )?,
                    rib_input: rib::RibInputTypeInfo::try_from(
                        value
                            .response_rib_input
                            .ok_or("Missing response_rib_input for Worker")?,
                    )?,
                    worker_calls: value
                        .worker_functions_in_response
                        .map(rib::WorkerFunctionsInRib::try_from)
                        .transpose()?,
                    rib_output: value
                        .response_rib_output
                        .map(RibOutputTypeInfo::try_from)
                        .transpose()?,
                };

                let binding_type = value
                    .binding_type
                    .unwrap_or(ProtoGatewayBindingType::Default.into());

                if binding_type == 0 {
                    Ok(GatewayBindingCompiled::Worker(WorkerBindingCompiled {
                        component_id,
                        worker_name_compiled,
                        idempotency_key_compiled,
                        response_compiled,
                        invocation_context_compiled,
                    }))
                } else {
                    Ok(GatewayBindingCompiled::FileServer(WorkerBindingCompiled {
                        component_id,
                        worker_name_compiled,
                        idempotency_key_compiled,
                        response_compiled,
                        invocation_context_compiled,
                    }))
                }
            }
            ProtoGatewayBindingType::HttpHandler => {
                // Convert fields for the Worker variant
                let component_id = value
                    .component
                    .ok_or("Missing component_id for Worker")?
                    .try_into()?;

                let worker_name_compiled = match (
                    value.worker_name,
                    value.compiled_worker_name_expr,
                    value.worker_name_rib_input,
                ) {
                    (Some(worker_name), Some(compiled_worker_name), Some(rib_input_type_info)) => {
                        Some(WorkerNameCompiled {
                            worker_name: rib::Expr::try_from(worker_name)?,
                            compiled_worker_name: rib::RibByteCode::try_from(compiled_worker_name)?,
                            rib_input_type_info: rib::RibInputTypeInfo::try_from(
                                rib_input_type_info,
                            )?,
                        })
                    }
                    _ => None,
                };

                let idempotency_key_compiled = match (
                    value.idempotency_key,
                    value.compiled_idempotency_key_expr,
                    value.idempotency_key_rib_input,
                ) {
                    (Some(idempotency_key), Some(compiled_idempotency_key), Some(rib_input)) => {
                        Some(IdempotencyKeyCompiled {
                            idempotency_key: rib::Expr::try_from(idempotency_key)?,
                            compiled_idempotency_key: rib::RibByteCode::try_from(
                                compiled_idempotency_key,
                            )?,
                            rib_input: rib::RibInputTypeInfo::try_from(rib_input)?,
                        })
                    }
                    _ => None,
                };

                Ok(GatewayBindingCompiled::HttpHandler(
                    HttpHandlerBindingCompiled {
                        component_id,
                        worker_name_compiled,
                        idempotency_key_compiled,
                    },
                ))
            }
            ProtoGatewayBindingType::CorsPreflight | ProtoGatewayBindingType::AuthCallBack => {
                let static_binding = value
                    .static_binding
                    .ok_or("Missing static_binding for Static")?;

                Ok(GatewayBindingCompiled::Static(static_binding.try_into()?))
            }
        }
    }
}

mod internal {
    use crate::gateway_binding::{HttpHandlerBindingCompiled, WorkerBindingCompiled};

    use golem_common::model::GatewayBindingType;

    pub(crate) fn worker_binding_to_gateway_binding_compiled_proto(
        worker_binding: WorkerBindingCompiled,
        binding_type: GatewayBindingType,
    ) -> Result<golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding, String> {
        let component = Some(worker_binding.component_id.into());
        let worker_name = worker_binding
            .worker_name_compiled
            .clone()
            .map(|w| w.worker_name.into());
        let compiled_worker_name_expr = worker_binding
            .worker_name_compiled
            .clone()
            .map(|w| w.compiled_worker_name.try_into())
            .transpose()?;
        let worker_name_rib_input = worker_binding
            .worker_name_compiled
            .map(|w| w.rib_input_type_info.into());
        let (idempotency_key, compiled_idempotency_key_expr, idempotency_key_rib_input) =
            match worker_binding.idempotency_key_compiled {
                Some(x) => (
                    Some(x.idempotency_key.into()),
                    Some(x.compiled_idempotency_key.try_into()?),
                    Some(x.rib_input.into()),
                ),
                None => (None, None, None),
            };

        let (invocation_context, compiled_invocation_context_expr, invocation_context_rib_input) =
            match worker_binding.invocation_context_compiled {
                Some(x) => (
                    Some(x.invocation_context.into()),
                    Some(x.compiled_invocation_context.try_into()?),
                    Some(x.rib_input.into()),
                ),
                None => (None, None, None),
            };

        let response = Some(
            worker_binding
                .response_compiled
                .response_mapping_expr
                .into(),
        );
        let compiled_response_expr = Some(
            worker_binding
                .response_compiled
                .response_mapping_compiled
                .try_into()?,
        );
        let response_rib_input = Some(worker_binding.response_compiled.rib_input.into());
        let response_rib_output = worker_binding
            .response_compiled
            .rib_output
            .map(golem_api_grpc::proto::golem::rib::RibOutputType::from);

        let worker_functions_in_response = worker_binding
            .response_compiled
            .worker_calls
            .map(|x| x.into());

        let binding_type = match binding_type {
            GatewayBindingType::Default => 0,
            GatewayBindingType::FileServer => 1,
            GatewayBindingType::CorsPreflight => 2,
            GatewayBindingType::HttpHandler => 4,
        };

        Ok(
            golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding {
                component,
                worker_name,
                compiled_worker_name_expr,
                worker_name_rib_input,
                idempotency_key,
                compiled_idempotency_key_expr,
                idempotency_key_rib_input,
                response,
                compiled_response_expr,
                response_rib_input,
                worker_functions_in_response,
                binding_type: Some(binding_type),
                static_binding: None,
                response_rib_output,
                invocation_context,
                compiled_invocation_context_expr,
                invocation_context_rib_input,
            },
        )
    }

    pub(crate) fn http_handler_to_gateway_binding_compiled_proto(
        http_handler_binding: HttpHandlerBindingCompiled,
        binding_type: GatewayBindingType,
    ) -> Result<golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding, String> {
        let component = Some(http_handler_binding.component_id.into());
        let worker_name = http_handler_binding
            .worker_name_compiled
            .clone()
            .map(|w| w.worker_name.into());
        let compiled_worker_name_expr = http_handler_binding
            .worker_name_compiled
            .clone()
            .map(|w| w.compiled_worker_name.try_into())
            .transpose()?;
        let worker_name_rib_input = http_handler_binding
            .worker_name_compiled
            .map(|w| w.rib_input_type_info.into());
        let (idempotency_key, compiled_idempotency_key_expr, idempotency_key_rib_input) =
            match http_handler_binding.idempotency_key_compiled {
                Some(x) => (
                    Some(x.idempotency_key.into()),
                    Some(x.compiled_idempotency_key.try_into()?),
                    Some(x.rib_input.into()),
                ),
                None => (None, None, None),
            };
        let binding_type = match binding_type {
            GatewayBindingType::Default => 0,
            GatewayBindingType::FileServer => 1,
            GatewayBindingType::CorsPreflight => 2,
            GatewayBindingType::HttpHandler => 4,
        };

        Ok(
            golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding {
                component,
                worker_name,
                compiled_worker_name_expr,
                worker_name_rib_input,
                idempotency_key,
                compiled_idempotency_key_expr,
                idempotency_key_rib_input,
                response: None,
                compiled_response_expr: None,
                response_rib_input: None,
                worker_functions_in_response: None,
                binding_type: Some(binding_type),
                static_binding: None,
                response_rib_output: None,
                invocation_context: None,
                compiled_invocation_context_expr: None,
                invocation_context_rib_input: None,
            },
        )
    }
}
