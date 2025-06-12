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

pub(crate) use self::http_handler_binding::*;
pub(crate) use self::worker_binding::*;
pub(crate) use crate::gateway_execution::gateway_binding_resolver::*;
use crate::gateway_rib_compiler::DefaultWorkerServiceRibCompiler;
use crate::gateway_rib_compiler::WorkerServiceRibCompiler;
pub(crate) use gateway_binding_compiled::*;
use golem_api_grpc::proto::golem::apidefinition::GatewayBindingType;
use golem_common::model::component::VersionedComponentId;
use golem_wasm_ast::analysis::AnalysedExport;
use rib::{Expr, RibByteCode, RibCompilationError, RibInputTypeInfo};
pub use static_binding::*;

mod gateway_binding_compiled;
mod http_handler_binding;
mod static_binding;
mod worker_binding;

// A gateway binding is integration to the backend. This is similar to AWS's x-amazon-gateway-integration
// where it holds the details of where to re-route.

// The default integration is `worker`
// Certain integrations can exist as a static binding, which is restricted
// from anything dynamic in nature. This implies, there will not be Rib in either pre-compiled or raw form.
#[derive(Debug, Clone, PartialEq)]
pub enum GatewayBinding {
    Default(WorkerBinding),
    FileServer(WorkerBinding),
    Static(StaticBinding),
    HttpHandler(HttpHandlerBinding),
}

impl GatewayBinding {
    pub fn is_http_cors_binding(&self) -> bool {
        match self {
            Self::Default(_) => false,
            Self::FileServer(_) => false,
            Self::HttpHandler(_) => false,
            Self::Static(s) => match s {
                StaticBinding::HttpCorsPreflight(_) => true,
                StaticBinding::HttpAuthCallBack(_) => false,
            },
        }
    }

    pub fn is_security_binding(&self) -> bool {
        match self {
            Self::Default(_) => false,
            Self::FileServer(_) => false,
            Self::HttpHandler(_) => false,
            Self::Static(s) => match s {
                StaticBinding::HttpCorsPreflight(_) => false,
                StaticBinding::HttpAuthCallBack(_) => true,
            },
        }
    }

    pub fn static_binding(value: StaticBinding) -> GatewayBinding {
        GatewayBinding::Static(value)
    }

    pub fn get_component_id(&self) -> Option<VersionedComponentId> {
        match self {
            Self::Default(worker_binding) => Some(worker_binding.component_id.clone()),
            Self::FileServer(worker_binding) => Some(worker_binding.component_id.clone()),
            Self::HttpHandler(http_handler_binding) => {
                Some(http_handler_binding.component_id.clone())
            }
            Self::Static(_) => None,
        }
    }
}

impl TryFrom<GatewayBinding> for golem_api_grpc::proto::golem::apidefinition::GatewayBinding {
    type Error = String;
    fn try_from(value: GatewayBinding) -> Result<Self, String> {
        match value {
            GatewayBinding::Default(worker_binding) => Ok(
                golem_api_grpc::proto::golem::apidefinition::GatewayBinding {
                    binding_type: Some(GatewayBindingType::Default.into()),
                    component: Some(worker_binding.component_id.into()),
                    worker_name: worker_binding.worker_name.map(|x| x.into()),
                    response: Some(worker_binding.response_mapping.0.into()),
                    idempotency_key: worker_binding.idempotency_key.map(|x| x.into()),
                    static_binding: None,
                    invocation_context: worker_binding.invocation_context.map(|x| x.into()),
                },
            ),
            GatewayBinding::FileServer(worker_binding) => Ok(
                golem_api_grpc::proto::golem::apidefinition::GatewayBinding {
                    binding_type: Some(GatewayBindingType::FileServer.into()),
                    component: Some(worker_binding.component_id.into()),
                    worker_name: worker_binding.worker_name.map(|x| x.into()),
                    response: Some(worker_binding.response_mapping.0.into()),
                    idempotency_key: worker_binding.idempotency_key.map(|x| x.into()),
                    static_binding: None,
                    invocation_context: None,
                },
            ),
            GatewayBinding::Static(static_binding) => {
                let static_binding =
                    golem_api_grpc::proto::golem::apidefinition::StaticBinding::try_from(
                        static_binding.clone(),
                    )?;

                let inner = static_binding
                    .static_binding
                    .clone()
                    .ok_or("Missing static binding")?;

                let gateway_binding_type: GatewayBindingType = match inner {
                    golem_api_grpc::proto::golem::apidefinition::static_binding::StaticBinding::HttpCorsPreflight(_) => GatewayBindingType::CorsPreflight,
                    golem_api_grpc::proto::golem::apidefinition::static_binding::StaticBinding::AuthCallback(_)  => GatewayBindingType::AuthCallBack,
                };

                Ok(
                    golem_api_grpc::proto::golem::apidefinition::GatewayBinding {
                        binding_type: Some(gateway_binding_type.into()),
                        component: None,
                        worker_name: None,
                        response: None,
                        idempotency_key: None,
                        static_binding: Some(static_binding),
                        invocation_context: None,
                    },
                )
            }
            GatewayBinding::HttpHandler(worker_binding) => Ok(
                golem_api_grpc::proto::golem::apidefinition::GatewayBinding {
                    binding_type: Some(GatewayBindingType::HttpHandler.into()),
                    component: Some(worker_binding.component_id.into()),
                    worker_name: worker_binding.worker_name.map(|x| x.into()),
                    response: None,
                    idempotency_key: worker_binding.idempotency_key.map(|x| x.into()),
                    static_binding: None,
                    invocation_context: None,
                },
            ),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::GatewayBinding> for GatewayBinding {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::GatewayBinding,
    ) -> Result<Self, Self::Error> {
        let binding_type_proto =
            golem_api_grpc::proto::golem::apidefinition::GatewayBindingType::try_from(
                value.binding_type.unwrap_or(0),
            )
            .map_err(|_| "Failed to convert binding type".to_string())?;

        match binding_type_proto {
            golem_api_grpc::proto::golem::apidefinition::GatewayBindingType::Default => {
                let component_id = VersionedComponentId::try_from(
                    value.component.ok_or("Missing component id".to_string())?,
                )?;
                let worker_name = value.worker_name.map(Expr::try_from).transpose()?;
                let idempotency_key = value.idempotency_key.map(Expr::try_from).transpose()?;
                let invocation_context =
                    value.invocation_context.map(Expr::try_from).transpose()?;
                let response_proto = value.response.ok_or("Missing response field")?;
                let response = Expr::try_from(response_proto)?;

                Ok(GatewayBinding::Default(WorkerBinding {
                    component_id,
                    worker_name,
                    idempotency_key,
                    response_mapping: ResponseMapping(response),
                    invocation_context,
                }))
            }
            golem_api_grpc::proto::golem::apidefinition::GatewayBindingType::FileServer => {
                let component_id = VersionedComponentId::try_from(
                    value.component.ok_or("Missing component id".to_string())?,
                )?;
                let worker_name = value.worker_name.map(Expr::try_from).transpose()?;
                let idempotency_key = value.idempotency_key.map(Expr::try_from).transpose()?;
                let response_proto = value.response.ok_or("Missing response field")?;
                let response = Expr::try_from(response_proto)?;

                Ok(GatewayBinding::FileServer(WorkerBinding {
                    component_id,
                    worker_name,
                    idempotency_key,
                    response_mapping: ResponseMapping(response),
                    invocation_context: None,
                }))
            }
            golem_api_grpc::proto::golem::apidefinition::GatewayBindingType::HttpHandler => {
                let component_id = VersionedComponentId::try_from(
                    value.component.ok_or("Missing component id".to_string())?,
                )?;
                let worker_name = value.worker_name.map(Expr::try_from).transpose()?;
                let idempotency_key = value.idempotency_key.map(Expr::try_from).transpose()?;

                Ok(GatewayBinding::HttpHandler(HttpHandlerBinding {
                    component_id,
                    worker_name,
                    idempotency_key,
                }))
            }
            golem_api_grpc::proto::golem::apidefinition::GatewayBindingType::CorsPreflight => {
                let static_binding = value.static_binding.ok_or("Missing static binding")?;

                Ok(GatewayBinding::static_binding(StaticBinding::try_from(
                    static_binding,
                )?))
            }

            golem_api_grpc::proto::golem::apidefinition::GatewayBindingType::AuthCallBack => {
                let static_binding = value.static_binding.ok_or("Missing static binding")?;

                Ok(GatewayBinding::static_binding(StaticBinding::try_from(
                    static_binding,
                )?))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerNameCompiled {
    pub worker_name: Expr,
    pub compiled_worker_name: RibByteCode,
    pub rib_input_type_info: RibInputTypeInfo,
}

impl WorkerNameCompiled {
    pub fn from_worker_name(
        worker_name: &Expr,
        exports: &[AnalysedExport],
    ) -> Result<Self, RibCompilationError> {
        let compiled_worker_name = DefaultWorkerServiceRibCompiler::compile(worker_name, exports)?;

        Ok(WorkerNameCompiled {
            worker_name: worker_name.clone(),
            compiled_worker_name: compiled_worker_name.byte_code,
            rib_input_type_info: compiled_worker_name.rib_input_type_info,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IdempotencyKeyCompiled {
    pub idempotency_key: Expr,
    pub compiled_idempotency_key: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}

impl IdempotencyKeyCompiled {
    pub fn from_idempotency_key(
        idempotency_key: &Expr,
        exports: &[AnalysedExport],
    ) -> Result<Self, RibCompilationError> {
        let idempotency_key_compiled =
            DefaultWorkerServiceRibCompiler::compile(idempotency_key, exports)?;

        Ok(IdempotencyKeyCompiled {
            idempotency_key: idempotency_key.clone(),
            compiled_idempotency_key: idempotency_key_compiled.byte_code,
            rib_input: idempotency_key_compiled.rib_input_type_info,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InvocationContextCompiled {
    pub invocation_context: Expr,
    pub compiled_invocation_context: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}

impl InvocationContextCompiled {
    pub fn from_invocation_context(
        invocation_context: &Expr,
        exports: &[AnalysedExport],
    ) -> Result<Self, RibCompilationError> {
        let invocation_context_compiled =
            DefaultWorkerServiceRibCompiler::compile(invocation_context, exports)?;

        Ok(InvocationContextCompiled {
            invocation_context: invocation_context.clone(),
            compiled_invocation_context: invocation_context_compiled.byte_code,
            rib_input: invocation_context_compiled.rib_input_type_info,
        })
    }
}
