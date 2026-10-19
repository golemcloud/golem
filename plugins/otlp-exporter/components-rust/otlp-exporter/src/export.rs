use crate::config::{ExporterConfig, ServiceNameMode};
use crate::helpers::{format_uuid, infer_span_kind};
use crate::otlp_json::{
    ExportLogsServiceRequest, ExportMetricsServiceRequest, ExportTraceServiceRequest, KeyValue,
    OtlpSpan, OtlpSpanLink, OtlpValue, SpanStatus,
};
use crate::state::{PendingSpan, ResourceIdentity};
use golem_rust::schema::wit::wire::{AgentId, ComponentId};
use golem_rust::wasip3::http::{client, types};
use golem_rust::wasip3::{wit_future, wit_stream};

pub(crate) fn build_otel_span(
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
    let kind = pending
        .kind
        .unwrap_or_else(|| infer_span_kind(&name, &pending.attributes));

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
        trace_id: pending.trace_id,
        span_id: pending.span_id,
        parent_span_id: pending.parent_span_id,
        name,
        kind,
        start_time_unix_nano: pending.start_time_ns.to_string(),
        end_time_unix_nano: end_time_ns.to_string(),
        attributes,
        links: pending
            .links
            .into_iter()
            .map(|link| OtlpSpanLink {
                trace_id: link.trace_id,
                span_id: link.span_id,
                trace_state: combined_trace_state(&link.trace_states),
            })
            .collect(),
        trace_state: combined_trace_state(&pending.trace_states),
        status,
    }
}

fn combined_trace_state(trace_states: &[String]) -> Option<String> {
    (!trace_states.is_empty()).then(|| trace_states.join(","))
}

fn build_service_name(
    config: &ExporterConfig,
    component_id: &ComponentId,
    worker_id: &AgentId,
) -> String {
    match config.service_name_mode {
        ServiceNameMode::AgentId => {
            let comp_uuid = format_uuid(component_id.uuid.high_bits, component_id.uuid.low_bits);
            format!("{}/{}", comp_uuid, worker_id.agent_id)
        }
        ServiceNameMode::AgentType => worker_id
            .agent_id
            .split_once('(')
            .map_or(worker_id.agent_id.as_str(), |(agent_type, _)| agent_type)
            .to_string(),
    }
}

pub(crate) fn build_resource_attributes(
    config: &ExporterConfig,
    component_id: &ComponentId,
    worker_id: &AgentId,
    metadata: &golem_rust::oplog_processor::host::AgentMetadata,
    identity: Option<&ResourceIdentity>,
) -> Vec<KeyValue> {
    let mut attributes = build_resource_attributes_for_revision(
        config,
        component_id,
        worker_id,
        metadata.component_revision,
    );
    if let Some(identity) = identity {
        append_identity_attributes(&mut attributes, identity);
    }
    attributes
}

fn append_identity_attributes(attributes: &mut Vec<KeyValue>, identity: &ResourceIdentity) {
    for (key, value) in [
        (
            "golem.agent.instance.id",
            format_uuid(identity.instance_id.0, identity.instance_id.1),
        ),
        (
            "golem.environment.id",
            format_uuid(identity.environment_id.0, identity.environment_id.1),
        ),
        ("golem.agent.mode", identity.agent_mode.to_string()),
        ("golem.agent.owner.kind", identity.owner_kind.to_string()),
    ] {
        attributes.push(KeyValue {
            key: key.to_string(),
            value: OtlpValue {
                string_value: value,
            },
        });
    }
}

fn build_resource_attributes_for_revision(
    config: &ExporterConfig,
    component_id: &ComponentId,
    worker_id: &AgentId,
    component_revision: u64,
) -> Vec<KeyValue> {
    let component_id_str = format_uuid(component_id.uuid.high_bits, component_id.uuid.low_bits);
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
                string_value: component_revision.to_string(),
            },
        },
    ]
}

async fn send_otlp_request(
    config: &ExporterConfig,
    path: &str,
    json: String,
) -> Result<(), String> {
    let url = format!("{}{}", config.endpoint.trim_end_matches('/'), path);

    let mut header_entries = vec![("content-type".to_string(), b"application/json".to_vec())];

    for (key, value) in &config.headers {
        header_entries.push((key.clone(), value.as_bytes().to_vec()));
    }

    let headers = types::Fields::from_list(&header_entries)
        .map_err(|err| format!("invalid OTLP headers: {err:?}"))?;

    let (mut body_tx, body_rx) = wit_stream::new();
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));

    let (request, transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
    request.set_method(&types::Method::Post).unwrap();
    set_request_uri(&request, &url)?;

    let (send_result, transmit_result, ()) = futures::join!(
        async { client::send(request).await },
        async { transmit.await },
        async {
            let remaining = body_tx.write_all(json.into_bytes()).await;
            assert!(remaining.is_empty());
            let _ = trailers_tx.write(Ok(None)).await;
            drop(body_tx);
        }
    );

    let response = send_result.map_err(|err| {
        let err = format!("OTLP transport error: {err:?}");
        eprintln!("{err}");
        err
    })?;

    transmit_result.map_err(|err| {
        let err = format!("OTLP request body error: {err:?}");
        eprintln!("{err}");
        err
    })?;

    let status = response.get_status_code();
    if !(200..300).contains(&status) {
        let err = format!("OTLP export failed with HTTP status: {status}");
        eprintln!("{err}");
        return Err(err);
    }

    Ok(())
}

fn set_request_uri(request: &types::Request, url: &str) -> Result<(), String> {
    let uri: http::Uri = url
        .parse()
        .map_err(|err| format!("invalid OTLP endpoint {url}: {err}"))?;
    match uri.scheme_str() {
        Some("http") => request.set_scheme(Some(&types::Scheme::Http)).unwrap(),
        Some("https") => request.set_scheme(Some(&types::Scheme::Https)).unwrap(),
        Some(scheme) => return Err(format!("unsupported OTLP endpoint scheme: {scheme}")),
        None => return Err(format!("OTLP endpoint must include a scheme: {url}")),
    }

    let authority = uri
        .authority()
        .ok_or_else(|| format!("OTLP endpoint must include an authority: {url}"))?;
    let path_with_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let normalized_path_with_query;
    let path_with_query = if path_with_query.starts_with('?') {
        normalized_path_with_query = format!("/{path_with_query}");
        normalized_path_with_query.as_str()
    } else {
        path_with_query
    };

    request.set_authority(Some(authority.as_str())).unwrap();
    request.set_path_with_query(Some(path_with_query)).unwrap();

    Ok(())
}

pub(crate) async fn send_spans(
    config: &ExporterConfig,
    request_body: ExportTraceServiceRequest,
) -> Result<(), String> {
    let json = serde_json::to_string(&request_body).map_err(|e| e.to_string())?;
    send_otlp_request(config, "/v1/traces", json).await
}

pub(crate) async fn send_logs(
    config: &ExporterConfig,
    request_body: ExportLogsServiceRequest,
) -> Result<(), String> {
    let json = serde_json::to_string(&request_body).map_err(|e| e.to_string())?;
    send_otlp_request(config, "/v1/logs", json).await
}

pub(crate) async fn send_metrics(
    config: &ExporterConfig,
    request_body: ExportMetricsServiceRequest,
) -> Result<(), String> {
    let json = serde_json::to_string(&request_body).map_err(|e| e.to_string())?;
    send_otlp_request(config, "/v1/metrics", json).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SignalConfig;
    use golem_rust::schema::wit::wire::Uuid;

    fn config(service_name_mode: ServiceNameMode) -> ExporterConfig {
        ExporterConfig {
            endpoint: "http://localhost:4318".to_string(),
            headers: Vec::new(),
            service_name_mode,
            signals: SignalConfig {
                traces: true,
                logs: false,
                metrics: false,
            },
        }
    }

    fn component_id() -> ComponentId {
        ComponentId {
            uuid: Uuid {
                high_bits: 0x0011223344556677,
                low_bits: 0x8899aabbccddeeff,
            },
        }
    }

    fn agent_id(value: &str) -> AgentId {
        AgentId {
            component_id: component_id(),
            agent_id: value.to_string(),
        }
    }

    fn attribute<'a>(attributes: &'a [KeyValue], key: &str) -> &'a str {
        &attributes
            .iter()
            .find(|attribute| attribute.key == key)
            .unwrap()
            .value
            .string_value
    }

    #[test]
    fn agent_type_service_name_ignores_constructor_parameters_and_phantom_id() {
        let first = agent_id("ShoppingCart(\"alice\", 1)");
        let second =
            agent_id("ShoppingCart(\"bob (preferred)\", 2)[00000000-0000-0000-0000-000000000001]");
        let config = config(ServiceNameMode::AgentType);

        assert_eq!(
            build_service_name(&config, &component_id(), &first),
            "ShoppingCart"
        );
        assert_eq!(
            build_service_name(&config, &component_id(), &second),
            "ShoppingCart"
        );
    }

    #[test]
    fn agent_type_service_name_accepts_external_tool_owners_without_an_agent_type() {
        let worker_id = agent_id("~golem-external-tool-owner-00000000-0000-0000-0000-000000000003");
        assert_eq!(
            build_service_name(
                &config(ServiceNameMode::AgentType),
                &component_id(),
                &worker_id
            ),
            worker_id.agent_id
        );
    }

    #[test]
    fn agent_id_service_name_and_resource_identity_attributes_are_unchanged() {
        let worker_id = agent_id("ShoppingCart(\"alice\")[00000000-0000-0000-0000-000000000002]");
        let component_id = component_id();
        let attributes = build_resource_attributes_for_revision(
            &config(ServiceNameMode::AgentId),
            &component_id,
            &worker_id,
            7,
        );
        let component = "00112233-4455-6677-8899-aabbccddeeff";

        assert_eq!(
            attribute(&attributes, "service.name"),
            format!("{component}/{}", worker_id.agent_id)
        );
        assert_eq!(
            attribute(&attributes, "golem.agent.id"),
            format!("{component}/{}", worker_id.agent_id)
        );
        assert_eq!(attribute(&attributes, "golem.component.id"), component);
    }

    #[test]
    fn create_identity_fields_become_bounded_resource_attributes() {
        let mut attributes = Vec::new();
        append_identity_attributes(
            &mut attributes,
            &ResourceIdentity {
                instance_id: (0x0011223344556677, 0x8899aabbccddeeff),
                environment_id: (0xffeeddccbbaa9988, 0x7766554433221100),
                agent_mode: "ephemeral",
                owner_kind: "ephemeral-external-tool",
            },
        );

        assert_eq!(
            attribute(&attributes, "golem.agent.instance.id"),
            "00112233-4455-6677-8899-aabbccddeeff"
        );
        assert_eq!(
            attribute(&attributes, "golem.environment.id"),
            "ffeeddcc-bbaa-9988-7766-554433221100"
        );
        assert_eq!(attribute(&attributes, "golem.agent.mode"), "ephemeral");
        assert_eq!(
            attribute(&attributes, "golem.agent.owner.kind"),
            "ephemeral-external-tool"
        );
    }
}
