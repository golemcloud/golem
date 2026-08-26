use futures_concurrency::future::Join;
use golem_rust::durability::{Durability, DurableFunctionType};
use golem_rust::{
    FromSchema, IntoSchema, agent_definition, agent_implementation, get_self_metadata,
};
use std::fmt::{Display, Formatter};

use crate::raw_http;
use crate::raw_http::Method;

#[derive(Debug, Clone, IntoSchema, FromSchema)]
struct StructuredInput {
    pub payload: String,
}

#[derive(Debug, Clone, IntoSchema, FromSchema)]
struct StructuredResult {
    pub result: String,
}

#[derive(Debug, IntoSchema, FromSchema)]
enum UnusedError {
    UnusedError,
}

impl Display for UnusedError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "UnusedError")
    }
}

#[agent_definition]
pub trait CustomDurability {
    fn new(name: String) -> Self;

    fn callback(&self, payload: String) -> String;

    fn callback_after_probe(&self, payload: String) -> String;

    async fn nested_no_input(&self) -> Vec<String>;

    async fn concurrent_no_input(&self) -> Vec<String>;
}

pub struct CustomDurabilityImpl {
    _name: String,
}

#[agent_implementation]
impl CustomDurability for CustomDurabilityImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn callback(&self, payload: String) -> String {
        self.durable_callback(payload, false)
    }

    fn callback_after_probe(&self, payload: String) -> String {
        self.durable_callback(payload, true)
    }

    async fn nested_no_input(&self) -> Vec<String> {
        let outer = durable_nested_generator().await;
        let root_sibling = durable_no_input_generator("root-sibling").await;
        vec![outer, root_sibling]
    }

    async fn concurrent_no_input(&self) -> Vec<String> {
        let (slow, fast) = (
            durable_no_input_generator("slow"),
            durable_no_input_generator("fast"),
        )
            .join()
            .await;
        vec![slow, fast]
    }
}

impl CustomDurabilityImpl {
    fn durable_callback(&self, payload: String, probe_first: bool) -> String {
        let input = StructuredInput {
            payload: payload.clone(),
        };
        Durability::<StructuredResult, UnusedError>::new(
            "golem-it",
            "test-callback",
            DurableFunctionType::WriteRemote,
            &input,
        )
        .run_infallible(|| {
            let result = {
                assert!(
                    !get_self_metadata()
                        .expect("self metadata access should be allowed")
                        .agent_id
                        .agent_id
                        .is_empty()
                );
                if probe_first {
                    perform_request("probe", payload.clone());
                }
                perform_callback(payload.clone())
            };
            StructuredResult { result }
        })
        .result
    }
}

async fn durable_nested_generator() -> String {
    Durability::<StructuredResult, UnusedError>::new(
        "golem-it",
        "test-generator-parent",
        DurableFunctionType::WriteRemote,
        &(),
    )
    .run_infallible_async(|| async {
        let result = durable_no_input_generator("nested").await;
        StructuredResult { result }
    })
    .await
    .result
}

async fn durable_no_input_generator(payload: &str) -> String {
    Durability::<StructuredResult, UnusedError>::new(
        "golem-it",
        "test-generator",
        DurableFunctionType::WriteRemote,
        &(),
    )
    .run_infallible_async(|| async {
        let result = {
            let path = format!("/callback?payload={payload}");
            let response = crate::raw_wasi_http::send_http_request(&path).await;
            String::from_utf8(crate::raw_wasi_http::read_body(response).await)
                .expect("Failed to read response text")
        };
        StructuredResult { result }
    })
    .await
    .result
}

fn perform_callback(payload: String) -> String {
    perform_request("callback", payload)
}

fn perform_request(endpoint: &str, payload: String) -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let authority = format!("localhost:{port}");
    let path = format!("/{endpoint}?payload={payload}");
    let (status, body) = raw_http::request(Method::Get, &authority, &path, None, None);
    assert_eq!(status, 200, "callback request failed with status {status}");
    String::from_utf8(body).expect("Failed to read response text")
}
