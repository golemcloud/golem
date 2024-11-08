pub(crate) use crate::gateway_binding::worker_binding::WorkerBinding;
pub(crate) use crate::gateway_execution::gateway_binding_resolver::*;
pub(crate) use crate::gateway_execution::rib_input_value_resolver::*;
pub(crate) use worker_binding_compiled::*;

use crate::gateway_middleware::Middlewares;
pub(crate) use gateway_binding_compiled::*;
use golem_service_base::model::VersionedComponentId;
use rib::Expr;
pub(crate) use static_binding::*;
pub(crate) use worker_binding::*;

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
    Worker(WorkerBinding),
    Static(StaticBinding),
}

impl GatewayBinding {
    pub fn get_worker_binding(&self) -> Option<WorkerBinding> {
        match self {
            Self::Worker(worker_binding) => Some(worker_binding.clone()),
            Self::Static(_) => None,
        }
    }
}

impl From<GatewayBinding> for golem_api_grpc::proto::golem::apidefinition::GatewayBinding {
    fn from(value: GatewayBinding) -> Self {
        match value {
            GatewayBinding::Worker(worker_binding) => {
                let middleware = worker_binding.middleware.map(|x| x.into());

                golem_api_grpc::proto::golem::apidefinition::GatewayBinding {
                    binding_type: Some(0),
                    component: Some(worker_binding.component_id.into()),
                    worker_name: worker_binding.worker_name.map(|x| x.into()),
                    response: Some(worker_binding.response.0.into()),
                    idempotency_key: worker_binding.idempotency_key.map(|x| x.into()),
                    middleware,
                    static_binding: None,
                }
            }
            GatewayBinding::Static(static_binding) => {
                golem_api_grpc::proto::golem::apidefinition::GatewayBinding {
                    binding_type: Some(1),
                    component: None,
                    worker_name: None,
                    response: None,
                    idempotency_key: None,
                    middleware: None,
                    static_binding: Some(static_binding.into()),
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

                Ok(GatewayBinding::Worker(WorkerBinding {
                    component_id,
                    worker_name,
                    idempotency_key,
                    response: ResponseMapping(response),
                    middleware,
                }))
            }
            golem_api_grpc::proto::golem::apidefinition::GatewayBindingType::Cors => {
                let static_binding = value.static_binding.ok_or("Missing static binding")?;

                Ok(GatewayBinding::Static(static_binding.try_into()?))
            }
        }
    }
}
