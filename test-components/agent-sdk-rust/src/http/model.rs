use golem_rust::{FromSchema, IntoSchema};

#[derive(IntoSchema, FromSchema)]
pub struct StringPathVarResponse {
    pub value: String,
}

#[derive(IntoSchema, FromSchema)]
pub struct MultiPathVarsResponse {
    pub joined: String,
}

#[derive(IntoSchema, FromSchema)]
pub struct RemainingPathResponse {
    pub tail: String,
}

#[derive(IntoSchema, FromSchema)]
pub struct PathAndQueryResponse {
    pub id: String,
    pub limit: u64,
}

#[derive(IntoSchema, FromSchema)]
pub struct PathAndHeaderResponse {
    pub resource_id: String,
    pub request_id: String,
}

#[derive(IntoSchema, FromSchema)]
pub struct JsonBodyResponse {
    pub ok: bool,
}

#[derive(IntoSchema, FromSchema)]
pub struct JsonResponse {
    pub value: String,
}

#[derive(IntoSchema, FromSchema)]
pub struct OptionalResponse {
    pub value: String,
}

#[derive(IntoSchema, FromSchema)]
pub struct ResultOkResponse {
    pub value: String,
}

#[derive(IntoSchema, FromSchema)]
pub struct ResultErrResponse {
    pub error: String,
}

#[derive(IntoSchema, FromSchema)]
pub struct PreflightResponse {
    pub received: String,
}

#[derive(IntoSchema, FromSchema)]
pub struct OkResponse {
    pub ok: bool,
}

#[derive(IntoSchema, FromSchema)]
pub struct PreflightRequest {
    pub name: String,
}

#[derive(IntoSchema, FromSchema)]
pub struct WebhookResponse {
    pub payload_length: u64,
}

// New response types for comprehensive HTTP method testing

#[derive(IntoSchema, FromSchema)]
pub struct ResourceUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(IntoSchema, FromSchema)]
pub struct ResourceResponse {
    pub id: String,
    pub updated: bool,
    pub method: String,
}

#[derive(IntoSchema, FromSchema)]
pub struct ResourceMetadata {
    pub id: String,
    pub exists: bool,
    pub content_length: Option<u64>,
}

#[derive(IntoSchema, FromSchema)]
pub struct OptionsResponse {
    pub allowed_methods: Vec<String>,
    pub allowed_headers: Vec<String>,
    pub max_age: u64,
}

#[derive(IntoSchema, FromSchema)]
pub struct ApiOptionsResponse {
    pub version: String,
    pub endpoints: Vec<String>,
}

#[derive(IntoSchema, FromSchema)]
pub struct TunnelResponse {
    pub host: String,
    pub port: u16,
    pub connected: bool,
}

#[derive(IntoSchema, FromSchema)]
pub struct ProxyResponse {
    pub target: String,
    pub proxy_active: bool,
}

#[derive(IntoSchema, FromSchema)]
pub struct TraceResponse {
    pub path: String,
    pub received_headers: Vec<String>,
    pub timestamp: u64,
}
