use golem_rust::{FromSchema, FromWire, IntoSchema, IntoWire, WireSchema};

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct StringPathVarResponse {
    pub value: String,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct MultiPathVarsResponse {
    pub joined: String,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct RemainingPathResponse {
    pub tail: String,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct PathAndQueryResponse {
    pub id: String,
    pub limit: u64,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct PathAndHeaderResponse {
    pub resource_id: String,
    pub request_id: String,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct JsonBodyResponse {
    pub ok: bool,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct JsonResponse {
    pub value: String,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct OptionalResponse {
    pub value: String,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct ResultOkResponse {
    pub value: String,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct ResultErrResponse {
    pub error: String,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct PreflightResponse {
    pub received: String,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct OkResponse {
    pub ok: bool,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct PreflightRequest {
    pub name: String,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct WebhookResponse {
    pub payload_length: u64,
}

// New response types for comprehensive HTTP method testing

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct ResourceUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct ResourceResponse {
    pub id: String,
    pub updated: bool,
    pub method: String,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct ResourceMetadata {
    pub id: String,
    pub exists: bool,
    pub content_length: Option<u64>,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct OptionsResponse {
    pub allowed_methods: Vec<String>,
    pub allowed_headers: Vec<String>,
    pub max_age: u64,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct ApiOptionsResponse {
    pub version: String,
    pub endpoints: Vec<String>,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct TunnelResponse {
    pub host: String,
    pub port: u16,
    pub connected: bool,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct ProxyResponse {
    pub target: String,
    pub proxy_active: bool,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct TraceResponse {
    pub path: String,
    pub received_headers: Vec<String>,
    pub timestamp: u64,
}
