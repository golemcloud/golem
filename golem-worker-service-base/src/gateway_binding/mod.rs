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

pub(crate) use crate::gateway_execution::gateway_binding_resolver::*;
pub(crate) use crate::gateway_execution::rib_input_value_resolver::*;
use crate::gateway_middleware::{
    HttpMiddleware, HttpRequestAuthentication, Middleware, Middlewares,
};
pub(crate) use crate::gateway_request::request_details::*;
use crate::gateway_security::SecuritySchemeWithProviderMetadata;
pub(crate) use gateway_binding_compiled::*;
use golem_service_base::model::VersionedComponentId;
use rib::Expr;
pub use static_binding::*;
use std::ops::Deref;
pub(crate) use worker_binding::*;
pub(crate) use worker_binding_compiled::*;

mod gateway_binding_compiled;
mod static_binding;
mod worker_binding;
mod worker_binding_compiled;
// A gateway binding is integration to the backend. This is similar to AWS's x-amazon-gateway-integration
// where it holds the details of where to re-route.

// The default integration is `worker`
// Certain integrations can exist as a static binding, which is restricted
// from anything dynamic in nature. This implies, there will not be Rib in either pre-compiled or raw form.
#[derive(Debug, Clone, PartialEq)]
pub enum GatewayBinding {
    Default(WorkerBinding),
    FileServer(WorkerBinding),
    Static(Box<StaticBinding>),
}

impl GatewayBinding {
    pub fn add_authenticate_request_middleware(
        &mut self,
        security_scheme: SecuritySchemeWithProviderMetadata,
    ) {
        match self {
            GatewayBinding::Static(_) => {}
            GatewayBinding::FileServer(worker_binding) => worker_binding.add_middleware(
                Middleware::Http(HttpMiddleware::authenticate_request(security_scheme)),
            ),
            GatewayBinding::Default(worker_binding) => worker_binding.add_middleware(
                Middleware::Http(HttpMiddleware::authenticate_request(security_scheme)),
            ),
        }
    }

    pub fn get_authenticate_request_middleware(&self) -> Option<HttpRequestAuthentication> {
        match self {
            GatewayBinding::Default(binding) => binding.get_auth_middleware(),
            GatewayBinding::FileServer(binding) => binding.get_auth_middleware(),
            GatewayBinding::Static(_) => None,
        }
    }
    pub fn is_http_cors_binding(&self) -> bool {
        match self {
            Self::Default(_) => false,
            Self::FileServer(_) => false,
            Self::Static(s) => match s.deref() {
                StaticBinding::HttpCorsPreflight(_) => true,
                StaticBinding::HttpAuthCallBack(_) => false,
            },
        }
    }

    pub fn static_binding(value: StaticBinding) -> GatewayBinding {
        GatewayBinding::Static(Box::new(value))
    }

    pub fn get_worker_binding(&self) -> Option<WorkerBinding> {
        match self {
            Self::Default(worker_binding) => Some(worker_binding.clone()),
            Self::FileServer(worker_binding) => Some(worker_binding.clone()),
            Self::Static(_) => None,
        }
    }

    pub fn get_worker_binding_mut(&mut self) -> Option<&mut WorkerBinding> {
        match self {
            Self::Default(worker_binding) => Some(worker_binding),
            Self::FileServer(worker_binding) => Some(worker_binding),
            Self::Static(_) => None,
        }
    }
}

impl From<GatewayBinding> for golem_api_grpc::proto::golem::apidefinition::GatewayBinding {
    fn from(value: GatewayBinding) -> Self {
        match value {
            GatewayBinding::Default(worker_binding) => {
                let middleware = worker_binding.middleware.map(|x| {
                    golem_api_grpc::proto::golem::apidefinition::Middleware::try_from(x).unwrap()
                });

                golem_api_grpc::proto::golem::apidefinition::GatewayBinding {
                    binding_type: Some(0),
                    component: Some(worker_binding.component_id.into()),
                    worker_name: worker_binding.worker_name.map(|x| x.into()),
                    response: Some(worker_binding.response_mapping.0.into()),
                    idempotency_key: worker_binding.idempotency_key.map(|x| x.into()),
                    middleware,
                    static_binding: None,
                }
            }
            GatewayBinding::FileServer(worker_binding) => {
                let middleware = worker_binding.middleware.map(|x| {
                    golem_api_grpc::proto::golem::apidefinition::Middleware::try_from(x).unwrap()
                });

                golem_api_grpc::proto::golem::apidefinition::GatewayBinding {
                    binding_type: Some(1),
                    component: Some(worker_binding.component_id.into()),
                    worker_name: worker_binding.worker_name.map(|x| x.into()),
                    response: Some(worker_binding.response_mapping.0.into()),
                    idempotency_key: worker_binding.idempotency_key.map(|x| x.into()),
                    middleware,
                    static_binding: None,
                }
            }
            GatewayBinding::Static(static_binding) => {
                golem_api_grpc::proto::golem::apidefinition::GatewayBinding {
                    binding_type: Some(2),
                    component: None,
                    worker_name: None,
                    response: None,
                    idempotency_key: None,
                    middleware: None,
                    static_binding: Some(static_binding.deref().clone().into()),
                }
            }
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
                let response_proto = value.response.ok_or("Missing response field")?;
                let response = Expr::try_from(response_proto)?;
                let middleware = value.middleware.map(Middlewares::try_from).transpose()?;

                Ok(GatewayBinding::Default(WorkerBinding {
                    component_id,
                    worker_name,
                    idempotency_key,
                    response_mapping: ResponseMapping(response),
                    middleware,
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
                let middleware = value.middleware.map(Middlewares::try_from).transpose()?;

                Ok(GatewayBinding::FileServer(WorkerBinding {
                    component_id,
                    worker_name,
                    idempotency_key,
                    response_mapping: ResponseMapping(response),
                    middleware,
                }))
            }
            golem_api_grpc::proto::golem::apidefinition::GatewayBindingType::CorsPreflight => {
                let static_binding = value.static_binding.ok_or("Missing static binding")?;

                Ok(GatewayBinding::static_binding(static_binding.try_into()?))
            }
        }
    }
}
