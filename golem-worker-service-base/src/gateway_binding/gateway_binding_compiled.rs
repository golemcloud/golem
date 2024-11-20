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

use crate::gateway_binding::StaticBinding;
use crate::gateway_binding::{
    GatewayBinding, IdempotencyKeyCompiled, ResponseMappingCompiled, WorkerBinding,
    WorkerBindingCompiled, WorkerNameCompiled,
};
use crate::gateway_middleware::Middlewares;
use golem_common::model::GatewayBindingType;

// A compiled binding is a binding with all existence of Rib Expr
// get replaced with their compiled form - RibByteCode.
#[derive(Debug, Clone, PartialEq)]
pub enum GatewayBindingCompiled {
    Worker(WorkerBindingCompiled),
    Static(Box<StaticBinding>),
    FileServer(WorkerBindingCompiled),
}

impl GatewayBindingCompiled {
    pub fn get_middlewares(&self) -> Option<Middlewares> {
        match self {
            GatewayBindingCompiled::Worker(worker_binding_compiled) => {
                worker_binding_compiled.middlewares.clone()
            }
            GatewayBindingCompiled::Static(_) => None,
            GatewayBindingCompiled::FileServer(worker_binding_compiled) => {
                worker_binding_compiled.middlewares.clone()
            }
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
        }
    }
}

impl From<GatewayBindingCompiled>
    for golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding
{
    fn from(value: GatewayBindingCompiled) -> Self {
        match value {
            GatewayBindingCompiled::Worker(worker_binding) => {
                internal::to_gateway_binding_compiled_proto(
                    worker_binding,
                    GatewayBindingType::Default,
                )
            }

            GatewayBindingCompiled::FileServer(worker_binding) => {
                internal::to_gateway_binding_compiled_proto(
                    worker_binding,
                    GatewayBindingType::FileServer,
                )
            }

            GatewayBindingCompiled::Static(static_binding) => {
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
                    binding_type: Some(1),
                    static_binding: Some(
                        golem_api_grpc::proto::golem::apidefinition::StaticBinding::from(
                            *static_binding,
                        ),
                    ),
                    middleware: None,
                }
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
        match value.binding_type {
            Some(0) | Some(1) => {
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
                };

                let middleware = value.middleware.map(Middlewares::try_from).transpose()?;

                let binding_type = value.binding_type.ok_or("Missing binding_type")?;

                if binding_type == 0 {
                    Ok(GatewayBindingCompiled::Worker(WorkerBindingCompiled {
                        component_id,
                        worker_name_compiled,
                        idempotency_key_compiled,
                        response_compiled,
                        middlewares: middleware,
                    }))
                } else {
                    Ok(GatewayBindingCompiled::FileServer(WorkerBindingCompiled {
                        component_id,
                        worker_name_compiled,
                        idempotency_key_compiled,
                        response_compiled,
                        middlewares: middleware,
                    }))
                }
            }
            Some(2) => {
                let static_binding = value
                    .static_binding
                    .ok_or("Missing static_binding for Static")?;

                Ok(GatewayBindingCompiled::Static(Box::new(
                    static_binding.try_into()?,
                )))
            }
            _ => Err("Unknown binding type".to_string()),
        }
    }
}

mod internal {
    use crate::gateway_binding::WorkerBindingCompiled;
    use crate::gateway_middleware::{HttpMiddleware, Middleware};
    use golem_api_grpc::proto::golem::apidefinition::SecurityWithProviderMetadata;
    use golem_common::model::GatewayBindingType;
    use std::ops::Deref;

    pub(crate) fn to_gateway_binding_compiled_proto(
        worker_binding: WorkerBindingCompiled,
        binding_type: GatewayBindingType,
    ) -> golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding {
        let component = Some(worker_binding.component_id.into());
        let worker_name = worker_binding
            .worker_name_compiled
            .clone()
            .map(|w| w.worker_name.into());
        let compiled_worker_name_expr = worker_binding
            .worker_name_compiled
            .clone()
            .map(|w| w.compiled_worker_name.into());
        let worker_name_rib_input = worker_binding
            .worker_name_compiled
            .map(|w| w.rib_input_type_info.into());
        let (idempotency_key, compiled_idempotency_key_expr, idempotency_key_rib_input) =
            match worker_binding.idempotency_key_compiled {
                Some(x) => (
                    Some(x.idempotency_key.into()),
                    Some(x.compiled_idempotency_key.into()),
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
                .into(),
        );
        let response_rib_input = Some(worker_binding.response_compiled.rib_input.into());
        let worker_functions_in_response = worker_binding
            .response_compiled
            .worker_calls
            .map(|x| x.into());

        let mut cors = None;
        let mut auth = None;

        let middleware = if let Some(m) = worker_binding.middlewares {
            for m in m.0.iter() {
                match m {
                    Middleware::Http(HttpMiddleware::AuthenticateRequest(auth0)) => {
                        let auth0 = auth0.deref().clone().security_scheme;
                        auth = Some(SecurityWithProviderMetadata::try_from(auth0).unwrap());
                    }
                    Middleware::Http(HttpMiddleware::AddCorsHeaders(cors0)) => {
                        let cors0 = cors0.clone();
                        cors = Some(
                            golem_api_grpc::proto::golem::apidefinition::CorsPreflight::from(cors0),
                        )
                    }
                }
            }
            Some(golem_api_grpc::proto::golem::apidefinition::Middleware {
                cors,
                http_authentication: auth,
            })
        } else {
            None
        };

        let binding_type = match binding_type {
            GatewayBindingType::Default => 0,
            GatewayBindingType::FileServer => 1,
            GatewayBindingType::CorsPreflight => 2,
        };

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
            middleware,
        }
    }
}
