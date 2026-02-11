use golem_rust::{agent_definition, agent_implementation, endpoint};
use golem_rust::bindings::golem::api::context::{current_context, start_span, AttributeValue};
use serde_json::{Value, json};
use golem_wasi_http::Client;
use std::collections::HashMap;

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
        Self {
            _name: name,
        }
    }

    async fn test1(&mut self) {
        broadcast_current_invocation_context("test1a");

        let mut client = InvocationContextAgentClient::get("w2".to_string());
        client.test2().await;
    }

    async fn test2(&mut self) {
        broadcast_current_invocation_context("test2a");

        let span = start_span("custom");
        span.set_attribute("x", &AttributeValue::String("1".to_string()));
        span.set_attribute("y", &AttributeValue::String("2".to_string()));

        let span2 = start_span("custom2");
        span2.set_attribute("z", &AttributeValue::String("3".to_string()));

        let mut client = InvocationContextAgentClient::get("w1".to_string());
        client.test3().await;
    }

    async fn test3(&mut self) {
        broadcast_current_invocation_context("test3a");
    }
}


fn broadcast_current_invocation_context(from: &str) {
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

    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let client = Client::builder().build().unwrap();

    client
        .post(&format!("http://localhost:{port}/invocation-context"))
        .json(&body)
        .send()
        .expect("Request failed");
}
