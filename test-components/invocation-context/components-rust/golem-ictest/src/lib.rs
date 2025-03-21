#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::golem::ictest_exports::golem_ictest_api::Guest;
use crate::bindings::golem::api::context::{current_context, start_span, AttributeValue};
use crate::bindings::golem::api::host::{resolve_worker_id, worker_uri};
use crate::bindings::golem::ictest_client::golem_ictest_client::GolemIctestApi;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

struct Component;

impl Guest for Component {
    fn test1() {
        broadcast_current_invocation_context("test1a");

        let api = GolemIctestApi::new(&worker_uri(
            &resolve_worker_id("golem_ictest", "w2").unwrap(),
        ));
        api.blocking_test2();
    }

    fn test2() {
        broadcast_current_invocation_context("test2a");

        let span = start_span("custom");
        span.set_attribute("x", &AttributeValue::String("1".to_string()));
        span.set_attribute("y", &AttributeValue::String("2".to_string()));

        let span2 = start_span("custom2");
        span2.set_attribute("z", &AttributeValue::String("3".to_string()));

        let api = GolemIctestApi::new(&worker_uri(
            &resolve_worker_id("golem_ictest", "w1").unwrap(),
        ));
        api.test3();
    }

    fn test3() {
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
            span.insert(attribute.key, match attribute.value {
                AttributeValue::String(s) => s
            });
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

bindings::export!(Component with_types_in bindings);
