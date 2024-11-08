use crate::gateway_binding::static_binding::StaticBinding;
use crate::gateway_binding::{
    GatewayBinding, IdempotencyKeyCompiled, ResponseMappingCompiled, WorkerBinding,
    WorkerBindingCompiled, WorkerNameCompiled,
};
use crate::gateway_middleware::{Middlewares};

// A compiled binding is a binding with all existence of Rib Expr
// get replaced with their compiled form - RibByteCode.
#[derive(Debug, Clone, PartialEq)]
pub enum GatewayBindingCompiled {
    Worker(WorkerBindingCompiled),
    Static(StaticBinding),
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

                GatewayBinding::Worker(worker_binding)
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

                let response = Some(worker_binding.response_compiled.response_rib_expr.into());
                let compiled_response_expr =
                    Some(worker_binding.response_compiled.compiled_response.into());
                let response_rib_input = Some(worker_binding.response_compiled.rib_input.into());
                let worker_functions_in_response = worker_binding
                    .response_compiled
                    .worker_calls
                    .map(|x| x.into());

                let middleware = worker_binding.middleware.iter().find_map(
                    |m| Some(golem_api_grpc::proto::golem::apidefinition::Middleware {
                        cors: m.get_cors().map(|x| x.into()),
                    })
                );

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
                    binding_type: Some(0),
                    static_binding: None,
                    middleware
                }
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
                    static_binding: Some(static_binding.into()),
                    middleware: None
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
            Some(0) => {
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
                    response_rib_expr: rib::Expr::try_from(
                        value.response.ok_or("Missing response for Worker")?,
                    )?,
                    compiled_response: rib::RibByteCode::try_from(
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

                let middleware = value.middleware.map(|m| {
                   Middlewares::try_from(m)
                }).transpose()?;

                Ok(GatewayBindingCompiled::Worker(WorkerBindingCompiled {
                    component_id,
                    worker_name_compiled,
                    idempotency_key_compiled,
                    response_compiled,
                    middleware
                }))
            }
            Some(1) => {
                let static_binding =
                    value.static_binding.ok_or("Missing static_binding for Static")?;

                Ok(GatewayBindingCompiled::Static(static_binding.try_into()?))
            }
            _ => Err("Unknown binding type".to_string()),
        }
    }
}
