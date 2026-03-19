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

#[derive(Clone, Serialize)]
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

#[derive(Clone, Serialize)]
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

// ── Logs (POST /v1/logs) ────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportLogsServiceRequest {
    pub(crate) resource_logs: Vec<ResourceLogs>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceLogs {
    pub(crate) resource: OtlpResource,
    pub(crate) scope_logs: Vec<ScopeLogs>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScopeLogs {
    pub(crate) scope: InstrumentationScope,
    pub(crate) log_records: Vec<OtlpLogRecord>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OtlpLogRecord {
    pub(crate) time_unix_nano: String,
    pub(crate) observed_time_unix_nano: String,
    pub(crate) severity_number: u32,
    pub(crate) severity_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) body: Option<OtlpValue>,
    pub(crate) attributes: Vec<KeyValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) span_id: Option<String>,
}

// ── Metrics (POST /v1/metrics) ──────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportMetricsServiceRequest {
    pub(crate) resource_metrics: Vec<ResourceMetrics>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceMetrics {
    pub(crate) resource: OtlpResource,
    pub(crate) scope_metrics: Vec<ScopeMetrics>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScopeMetrics {
    pub(crate) scope: InstrumentationScope,
    pub(crate) metrics: Vec<OtlpMetric>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OtlpMetric {
    pub(crate) name: String,
    pub(crate) unit: String,
    pub(crate) description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sum: Option<OtlpSum>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) gauge: Option<OtlpGauge>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OtlpSum {
    pub(crate) aggregation_temporality: u32,
    pub(crate) is_monotonic: bool,
    pub(crate) data_points: Vec<OtlpNumberDataPoint>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OtlpGauge {
    pub(crate) data_points: Vec<OtlpNumberDataPoint>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OtlpNumberDataPoint {
    pub(crate) start_time_unix_nano: String,
    pub(crate) time_unix_nano: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) as_int: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) as_double: Option<f64>,
    pub(crate) attributes: Vec<KeyValue>,
}
