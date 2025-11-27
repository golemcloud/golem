use super::HttpCors;
use super::rib_compiler::{ComponentDependencyWithAgentInfo, compile_rib};
use super::security_scheme::SecuritySchemeWithProviderMetadata;
use desert_rust::BinaryCodec;
use golem_common::model::component::{ComponentId, ComponentRevision};
use rib::{
    Expr, RibByteCode, RibCompilationError, RibInputTypeInfo, RibOutputTypeInfo,
    WorkerFunctionsInRib,
};

#[derive(Debug, Clone, PartialEq)]
pub enum GatewayBindingCompiled {
    HttpCorsPreflight(HttpCors),
    HttpAuthCallBack(Box<SecuritySchemeWithProviderMetadata>),
    Worker(Box<WorkerBindingCompiled>),
    FileServer(Box<FileServerBindingCompiled>),
    HttpHandler(Box<HttpHandlerBindingCompiled>),
    SwaggerUi(SwaggerUiBinding),
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct WorkerBindingCompiled {
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
    pub idempotency_key_compiled: Option<IdempotencyKeyCompiled>,
    pub invocation_context_compiled: Option<InvocationContextCompiled>,
    pub response_compiled: ResponseMappingCompiled,
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct FileServerBindingCompiled {
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
    pub worker_name_compiled: WorkerNameCompiled,
    pub response_compiled: ResponseMappingCompiled,
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct HttpHandlerBindingCompiled {
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
    pub worker_name_compiled: WorkerNameCompiled,
    pub idempotency_key_compiled: Option<IdempotencyKeyCompiled>,
    pub invocation_context_compiled: Option<InvocationContextCompiled>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwaggerUiBinding {
    pub openapi_spec_json: Option<String>,
}

impl SwaggerUiBinding {
    pub fn empty() -> Self {
        Self {
            openapi_spec_json: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct ResponseMappingCompiled {
    pub response_mapping_expr: Expr,
    pub response_mapping_compiled: RibByteCode,
    pub rib_input: RibInputTypeInfo,
    pub worker_calls: Option<WorkerFunctionsInRib>,
    // Optional to keep backward compatibility
    pub rib_output: Option<RibOutputTypeInfo>,
}

impl ResponseMappingCompiled {
    pub fn from_expr(
        expr: &Expr,
        component_dependency: &[ComponentDependencyWithAgentInfo],
    ) -> Result<Self, RibCompilationError> {
        let response_compiled = compile_rib(expr, component_dependency)?;

        Ok(ResponseMappingCompiled {
            response_mapping_expr: expr.clone(),
            response_mapping_compiled: response_compiled.byte_code,
            rib_input: response_compiled.rib_input_type_info,
            worker_calls: response_compiled.worker_invoke_calls,
            rib_output: response_compiled.rib_output_type_info,
        })
    }
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct WorkerNameCompiled {
    pub worker_name: Expr,
    pub compiled_worker_name: RibByteCode,
    pub rib_input_type_info: RibInputTypeInfo,
}

impl WorkerNameCompiled {
    pub fn from_expr(expr: &Expr) -> Result<Self, RibCompilationError> {
        let compiled_worker_name = compile_rib(expr, &[])?;

        Ok(WorkerNameCompiled {
            worker_name: expr.clone(),
            compiled_worker_name: compiled_worker_name.byte_code,
            rib_input_type_info: compiled_worker_name.rib_input_type_info,
        })
    }
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct IdempotencyKeyCompiled {
    pub idempotency_key: Expr,
    pub compiled_idempotency_key: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}

impl IdempotencyKeyCompiled {
    pub fn from_expr(expr: &Expr) -> Result<Self, RibCompilationError> {
        let idempotency_key_compiled = compile_rib(expr, &[])?;

        Ok(IdempotencyKeyCompiled {
            idempotency_key: expr.clone(),
            compiled_idempotency_key: idempotency_key_compiled.byte_code,
            rib_input: idempotency_key_compiled.rib_input_type_info,
        })
    }
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct InvocationContextCompiled {
    pub invocation_context: Expr,
    pub compiled_invocation_context: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}

impl InvocationContextCompiled {
    pub fn from_expr(
        expr: &Expr,
        exports: &[ComponentDependencyWithAgentInfo],
    ) -> Result<Self, RibCompilationError> {
        let invocation_context_compiled = compile_rib(expr, exports)?;

        Ok(InvocationContextCompiled {
            invocation_context: expr.clone(),
            compiled_invocation_context: invocation_context_compiled.byte_code,
            rib_input: invocation_context_compiled.rib_input_type_info,
        })
    }
}
