// We intentionally use small manual OTLP/HTTP JSON structs rather than `opentelemetry-proto`,
// because this plugin emits OTLP JSON (not protobuf) and the generated proto types are a poor
// fit for the JSON wire format in this WASM build.

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportTraceServiceRequest {
    pub(crate) resource_spans: Vec<ResourceSpans>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceSpans {
    pub(crate) resource: OtlpResource,
    pub(crate) scope_spans: Vec<ScopeSpans>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OtlpResource {
    pub(crate) attributes: Vec<KeyValue>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScopeSpans {
    pub(crate) scope: InstrumentationScope,
    pub(crate) spans: Vec<OtlpSpan>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstrumentationScope {
    pub(crate) name: String,
    pub(crate) version: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OtlpSpan {
    pub(crate) trace_id: String,
    pub(crate) span_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parent_span_id: Option<String>,
    pub(crate) name: String,
    pub(crate) kind: u32,
    pub(crate) start_time_unix_nano: String,
    pub(crate) end_time_unix_nano: String,
    pub(crate) attributes: Vec<KeyValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) trace_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<SpanStatus>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SpanStatus {
    pub(crate) code: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KeyValue {
    pub(crate) key: String,
    pub(crate) value: OtlpValue,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OtlpValue {
    pub(crate) string_value: String,
}
