use golem_rust::Schema;

#[derive(Schema)]
pub struct StringPathVarResponse {
    pub value: String,
}

#[derive(Schema)]
pub struct MultiPathVarsResponse {
    pub joined: String,
}

#[derive(Schema)]
pub struct RemainingPathResponse {
    pub tail: String,
}

#[derive(Schema)]
pub struct PathAndQueryResponse {
    pub id: String,
    pub limit: u64,
}

#[derive(Schema)]
pub struct PathAndHeaderResponse {
    pub resource_id: String,
    pub request_id: String,
}

#[derive(Schema)]
pub struct JsonBodyResponse {
    pub ok: bool,
}

#[derive(Schema)]
pub struct JsonResponse {
    pub value: String,
}

#[derive(Schema)]
pub struct OptionalResponse {
    pub value: String,
}

#[derive(Schema)]
pub struct ResultOkResponse {
    pub value: String,
}

#[derive(Schema)]
pub struct ResultErrResponse {
    pub error: String,
}

#[derive(Schema)]
pub struct PreflightResponse {
    pub received: String,
}

#[derive(Schema)]
pub struct OkResponse {
    pub ok: bool,
}

#[derive(Schema)]
pub struct PreflightRequest {
    pub name: String,
}

#[derive(Schema)]
pub struct WebhookResponse {
    pub payload_length: u64,
}

// New response types for comprehensive HTTP method testing

#[derive(Schema)]
pub struct ResourceUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Schema)]
pub struct ResourceResponse {
    pub id: String,
    pub updated: bool,
    pub method: String,
}

#[derive(Schema)]
pub struct ResourceMetadata {
    pub id: String,
    pub exists: bool,
    pub content_length: Option<u64>,
}

#[derive(Schema)]
pub struct OptionsResponse {
    pub allowed_methods: Vec<String>,
    pub allowed_headers: Vec<String>,
    pub max_age: u64,
}

#[derive(Schema)]
pub struct ApiOptionsResponse {
    pub version: String,
    pub endpoints: Vec<String>,
}

#[derive(Schema)]
pub struct TunnelResponse {
    pub host: String,
    pub port: u16,
    pub connected: bool,
}

#[derive(Schema)]
pub struct ProxyResponse {
    pub target: String,
    pub proxy_active: bool,
}

#[derive(Schema)]
pub struct TraceResponse {
    pub path: String,
    pub received_headers: Vec<String>,
    pub timestamp: u64,
}
