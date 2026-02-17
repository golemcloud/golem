use golem_rust::bindings::golem::api::context::{current_context, start_span, AttributeValue};
use golem_rust::{agent_definition, agent_implementation};
use golem_wasi_http::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

#[agent_definition]
pub trait InvocationContext {
    fn new(name: String) -> Self;
    async fn test1(&self);
    fn test2(&self);
    fn test3(&self);
}

pub struct InvocationContextImpl {
    _name: String,
}

#[agent_implementation]
impl InvocationContext for InvocationContextImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    async fn test1(&self) {
        broadcast_current_invocation_context("test1a");

        let other = InvocationContextClient::get("w2".to_string());
        other.test2().await;
    }

    fn test2(&self) {
        broadcast_current_invocation_context("test2a");

        let span = start_span("custom");
        span.set_attribute("x", &AttributeValue::String("1".to_string()));
        span.set_attribute("y", &AttributeValue::String("2".to_string()));

        let _span2 = start_span("custom2");
        _span2.set_attribute("z", &AttributeValue::String("3".to_string()));

        let other = InvocationContextClient::get("w1".to_string());
        other.trigger_test3();
    }

    fn test3(&self) {
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
