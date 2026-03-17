use crate::config::{ExporterConfig, ServiceNameMode};
use crate::helpers::{format_uuid, infer_span_kind};
use crate::otlp_json::{
    ExportTraceServiceRequest, KeyValue, OtlpSpan, OtlpValue, SpanStatus,
};
use crate::state::PendingSpan;
use golem_rust::golem_wasm::golem_core_1_5_x::types::{AgentId, ComponentId};
use wstd::http::{Body, Client, Request};
use wstd::runtime::block_on;

pub(crate) fn build_otel_span(
    trace_id: &str,
    trace_state: Option<&str>,
    mut pending: PendingSpan,
    end_time_ns: u128,
    is_error: bool,
    error_message: Option<&str>,
) -> OtlpSpan {
    let name = pending
        .attributes
        .remove("name")
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            match (
                pending.attributes.get("request.method"),
                pending.attributes.get("request.uri"),
            ) {
                (Some(method), Some(uri)) if !method.is_empty() && !uri.is_empty() => {
                    Some(format!("{method} {uri}"))
                }
                _ => None,
            }
        })
        .unwrap_or_else(|| "unknown".to_string());
    let kind = infer_span_kind(&name, &pending.attributes);

    let attributes: Vec<KeyValue> = pending
        .attributes
        .into_iter()
        .map(|(key, value)| KeyValue {
            key,
            value: OtlpValue {
                string_value: value,
            },
        })
        .collect();

    let status = if is_error {
        Some(SpanStatus {
            code: 2, // ERROR
            message: error_message.map(|s| s.to_string()),
        })
    } else {
        None
    };

    OtlpSpan {
        trace_id: trace_id.to_string(),
        span_id: pending.span_id,
        parent_span_id: pending.parent_span_id,
        name,
        kind,
        start_time_unix_nano: pending.start_time_ns.to_string(),
        end_time_unix_nano: end_time_ns.to_string(),
        attributes,
        trace_state: trace_state.map(|s| s.to_string()),
        status,
    }
}

fn build_service_name(
    config: &ExporterConfig,
    component_id: &ComponentId,
    worker_id: &AgentId,
) -> String {
    match config.service_name_mode {
        ServiceNameMode::AgentId => {
            let comp_uuid =
                format_uuid(component_id.uuid.high_bits, component_id.uuid.low_bits);
            format!("{}/{}", comp_uuid, worker_id.agent_id)
        }
        ServiceNameMode::AgentType => worker_id.agent_id.clone(),
    }
}

pub(crate) fn build_resource_attributes(
    config: &ExporterConfig,
    component_id: &ComponentId,
    worker_id: &AgentId,
    metadata: &golem_rust::bindings::golem::api::host::AgentMetadata,
) -> Vec<KeyValue> {
    let component_id_str =
        format_uuid(component_id.uuid.high_bits, component_id.uuid.low_bits);
    let agent_id_str = format!("{}/{}", component_id_str, worker_id.agent_id);
    let service_name = build_service_name(config, component_id, worker_id);

    vec![
        KeyValue {
            key: "service.name".to_string(),
            value: OtlpValue {
                string_value: service_name,
            },
        },
        KeyValue {
            key: "golem.agent.id".to_string(),
            value: OtlpValue {
                string_value: agent_id_str,
            },
        },
        KeyValue {
            key: "golem.component.id".to_string(),
            value: OtlpValue {
                string_value: component_id_str,
            },
        },
        KeyValue {
            key: "golem.component.version".to_string(),
            value: OtlpValue {
                string_value: metadata.component_revision.to_string(),
            },
        },
    ]
}

pub(crate) fn send_spans(
    config: &ExporterConfig,
    request_body: ExportTraceServiceRequest,
) -> Result<(), String> {
    let json = serde_json::to_string(&request_body).map_err(|e| e.to_string())?;

    block_on(async {
        let url = format!(
            "{}/v1/traces",
            config.endpoint.trim_end_matches('/')
        );

        let mut builder = Request::post(&url)
            .header("Content-Type", "application/json");

        for (key, value) in &config.headers {
            builder = builder.header(key.as_str(), value.as_str());
        }

        let body = Body::from(json.into_bytes());
        let request = builder.body(body).map_err(|e| e.to_string())?;

        let response = Client::new()
            .send(request)
            .await
            .map_err(|e| format!("OTLP transport error: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!("OTLP export failed with HTTP status: {status}"));
        }

        Ok(())
    })
}
