use super::HttpCors;
use super::security_scheme::SecuritySchemeWithProviderMetadata;
use golem_common::model::component::{ComponentId, ComponentRevision};
use rib::{Expr, RibByteCode, RibInputTypeInfo, RibOutputTypeInfo, WorkerFunctionsInRib};

#[derive(Debug, Clone, PartialEq)]
pub enum GatewayBindingCompiled {
    Static(StaticBinding),
    Worker(Box<WorkerBindingCompiled>),
    FileServer(Box<FileServerBindingCompiled>),
    HttpHandler(Box<HttpHandlerBindingCompiled>),
    SwaggerUi(SwaggerUiBinding),
}

// Static bindings must NOT contain Rib, in either pre-compiled or raw form,
// as it may introduce unnecessary latency
// in serving the requests when not needed.
// Example of a static binding is a pre-flight request which can be handled by CorsPreflight
// Example: browser requests for preflights need only what's contained in a pre-flight CORS middleware and
// don't need to pass through to the backend.
#[derive(Debug, Clone, PartialEq)]
pub enum StaticBinding {
    HttpCorsPreflight(HttpCors),
    HttpAuthCallBack(Box<SecuritySchemeWithProviderMetadata>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerBindingCompiled {
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
    pub idempotency_key_compiled: Option<IdempotencyKeyCompiled>,
    pub response_compiled: ResponseMappingCompiled,
    pub invocation_context_compiled: Option<InvocationContextCompiled>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileServerBindingCompiled {
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
    pub worker_name_compiled: Option<WorkerNameCompiled>,
    pub idempotency_key_compiled: Option<IdempotencyKeyCompiled>,
    pub response_compiled: ResponseMappingCompiled,
    pub invocation_context_compiled: Option<InvocationContextCompiled>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpHandlerBindingCompiled {
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
    pub worker_name_compiled: Option<WorkerNameCompiled>,
    pub idempotency_key_compiled: Option<IdempotencyKeyCompiled>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwaggerUiBinding {
    pub openapi_spec_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResponseMappingCompiled {
    pub response_mapping_expr: Expr,
    pub response_mapping_compiled: RibByteCode,
    pub rib_input: RibInputTypeInfo,
    pub worker_calls: Option<WorkerFunctionsInRib>,
    // Optional to keep backward compatibility
    pub rib_output: Option<RibOutputTypeInfo>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerNameCompiled {
    pub worker_name: Expr,
    pub compiled_worker_name: RibByteCode,
    pub rib_input_type_info: RibInputTypeInfo,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IdempotencyKeyCompiled {
    pub idempotency_key: Expr,
    pub compiled_idempotency_key: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InvocationContextCompiled {
    pub invocation_context: Expr,
    pub compiled_invocation_context: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}
