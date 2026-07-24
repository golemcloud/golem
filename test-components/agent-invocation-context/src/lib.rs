use golem_rust::bindings::golem::api::context::{AttributeValue, current_context, start_span};
use golem_rust::{agent_definition, agent_implementation, endpoint};
use serde_json::{Value, json};
use std::collections::HashMap;
use wasip3::http::{client, types};
use wasip3::{wit_future, wit_stream};

#[agent_definition(mount = "/{name}")]
pub trait InvocationContextAgent {
    fn new(name: String) -> Self;

    #[endpoint(post = "/test-path-1")]
    async fn test1(&mut self);
    async fn test2(&mut self);
    async fn test3(&mut self);
}

struct InvocationContextAgentImpl {
    _name: String,
}

#[agent_implementation]
impl InvocationContextAgent for InvocationContextAgentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    async fn test1(&mut self) {
        broadcast_current_invocation_context("test1a").await;

        let mut client = InvocationContextAgentClient::get("w2".to_string());
        client.test2().await;
    }

    async fn test2(&mut self) {
        broadcast_current_invocation_context("test2a").await;

        let span = start_span("custom");
        span.set_attribute("x", &AttributeValue::String("1".to_string()));
        span.set_attribute("y", &AttributeValue::String("2".to_string()));

        let span2 = start_span("custom2");
        span2.set_attribute("z", &AttributeValue::String("3".to_string()));

        let mut client = InvocationContextAgentClient::get("w1".to_string());
        client.test3().await;
    }

    async fn test3(&mut self) {
        broadcast_current_invocation_context("test3a").await;
    }
}

async fn broadcast_current_invocation_context(from: &str) {
    let ctx = current_context();

    let trace_id = ctx.trace_id();
    let span_id = ctx.span_id();
    let trace_context_headers = Value::Object(
        ctx.trace_context_headers()
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect(),
    );

    let mut spans = Vec::new();
    let mut current = ctx;
    loop {
        let attributes = current.get_attributes(false);
        let mut span = HashMap::new();
        for attribute in attributes {
            span.insert(
                attribute.key,
                match attribute.value {
                    AttributeValue::String(s) => s,
                },
            );
        }
        spans.push(span);
        if let Some(parent) = current.parent() {
            current = parent;
        } else {
            break;
        }
    }

    let body = json!({
        "from": from,
        "trace_id": trace_id,
        "span_id": span_id,
        "spans": spans,
        "headers": trace_context_headers
    });
    println!("Sending context {body} through HTTP");

    let Ok(port) = std::env::var("PORT") else {
        return;
    };
    let body = body.to_string().into_bytes();

    match send_p3_http_post(&port, "/invocation-context", body).await {
        Ok(_) => println!("Context broadcast succeeded"),
        Err(e) => println!("Context broadcast failed (non-fatal): {e:?}"),
    }
}

async fn send_p3_http_post(
    port: &str,
    path_with_query: &str,
    body: Vec<u8>,
) -> Result<(), types::ErrorCode> {
    let headers =
        types::Fields::from_list(&[("content-type".to_string(), b"application/json".to_vec())])
            .expect("valid HTTP headers");

    let (mut body_tx, body_rx) = wit_stream::new();
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));

    let options = types::RequestOptions::new();
    options.set_connect_timeout(Some(5_000_000_000)).unwrap();
    options.set_first_byte_timeout(Some(5_000_000_000)).unwrap();
    options
        .set_between_bytes_timeout(Some(5_000_000_000))
        .unwrap();

    let (request, transmit) =
        types::Request::new(headers, Some(body_rx), trailers_rx, Some(options));
    request.set_method(&types::Method::Post).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();
    request.set_path_with_query(Some(path_with_query)).unwrap();

    let (send_result, transmit_result, ()) = futures::join!(
        async { client::send(request).await },
        async { transmit.await },
        async {
            let remaining = body_tx.write_all(body).await;
            assert!(remaining.is_empty());
            let _ = trailers_tx.write(Ok(None)).await;
            drop(body_tx);
        }
    );

    let response = send_result?;
    drop(response);
    transmit_result
}
