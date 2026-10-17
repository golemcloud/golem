use futures_concurrency::future::{Join, Race};
use golem_rust::durability::{Durability, DurableFunctionType};
use golem_rust::{
    FromSchema, IntoSchema, agent_definition, agent_implementation, get_self_metadata,
};
use std::fmt::{Display, Formatter};

use crate::raw_http;
use crate::raw_http::Method;

mod context_bindings {
    wit_bindgen::generate!({
        path: "../../wit",
        inline: "package test:custom-context; world context-test { import golem:durability/durability@1.6.0; }",
        generate_all,
        with: {
            "golem:core/types@2.0.0": golem_rust::schema::wit::wire,
        },
    });
}

use context_bindings::golem::durability::durability as raw_context;

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

    fn warmup_environment(&self);

    fn create_promise(&self) -> golem_rust::PromiseId;

    async fn concurrent_promises(
        &self,
        first: golem_rust::PromiseId,
        second: golem_rust::PromiseId,
    ) -> Vec<String>;

    async fn nested_no_input(&self) -> Vec<String>;

    async fn concurrent_no_input(&self) -> Vec<String>;

    async fn concurrent_first_finishes(&self) -> Vec<String>;

    async fn concurrent_nested(&self) -> Vec<String>;

    async fn scope_lifecycle(&self) -> Vec<String>;

    async fn cancel_pending_result(&self, fallible: bool);

    fn context_probe(&mut self, mode: String);
}

pub struct CustomDurabilityImpl {
    _name: String,
    saved_context: Option<raw_context::CustomInvocationContext>,
}

#[agent_implementation]
impl CustomDurability for CustomDurabilityImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            saved_context: None,
        }
    }

    fn callback(&self, payload: String) -> String {
        self.durable_callback(payload, false)
    }

    fn callback_after_probe(&self, payload: String) -> String {
        self.durable_callback(payload, true)
    }

    fn warmup_environment(&self) {
        std::env::var("PORT").unwrap();
    }

    fn create_promise(&self) -> golem_rust::PromiseId {
        golem_rust::create_promise()
    }

    async fn concurrent_promises(
        &self,
        first: golem_rust::PromiseId,
        second: golem_rust::PromiseId,
    ) -> Vec<String> {
        let (first, second) = (durable_promise(&first), durable_promise(&second))
            .join()
            .await;
        // This root call must keep its identity after either sibling reconstructs.
        let _ = golem_rust::generate_idempotency_key();
        vec![first, second]
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

    async fn concurrent_first_finishes(&self) -> Vec<String> {
        let (first, second) = (
            async {
                let first = durable_no_input_generator("first").await;
                let response = crate::raw_wasi_http::send_http_request("/release").await;
                crate::raw_wasi_http::read_body(response).await;
                first
            },
            durable_no_input_generator("second"),
        )
            .join()
            .await;
        vec![first, second]
    }

    async fn concurrent_nested(&self) -> Vec<String> {
        let (first, second) = (durable_nested_generator(), durable_nested_generator())
            .join()
            .await;
        vec![first, second]
    }

    async fn scope_lifecycle(&self) -> Vec<String> {
        let prepared = Durability::<String, UnusedError>::new(
            "golem-it",
            "prepared",
            DurableFunctionType::WriteRemote,
            &(),
        );
        // Creating and dropping an unpolled runner must not open a scope.
        drop(
            Durability::<String, UnusedError>::new(
                "golem-it",
                "unpolled",
                DurableFunctionType::WriteRemote,
                &(),
            )
            .run_infallible_async::<String, _, _>(|| async { panic!("unpolled body ran") }),
        );
        let local = std::rc::Rc::new("prepared".to_string());
        let prepared_future = prepared.run_infallible_async(|| async { local.as_ref().clone() });
        let prepared_result = Durability::<String, UnusedError>::new(
            "golem-it",
            "prepared-parent",
            DurableFunctionType::WriteRemote,
            &(),
        )
        .run_infallible_async(|| prepared_future)
        .await;

        let body_started = std::cell::Cell::new(false);
        let mut abandoned = Box::pin(
            Durability::<String, UnusedError>::new(
                "golem-it",
                "abandoned",
                DurableFunctionType::WriteRemote,
                &(),
            )
            .run_infallible_async(|| async {
                Durability::<String, UnusedError>::new(
                    "golem-it",
                    "abandoned-child",
                    DurableFunctionType::WriteRemote,
                    &(),
                )
                .run_infallible_async(|| async {
                    let _on_drop = DropRequest("cancel-drop");
                    body_started.set(true);
                    std::future::pending::<String>().await
                })
                .await
            }),
        );
        std::future::poll_fn(|cx| {
            assert!(abandoned.as_mut().poll(cx).is_pending());
            if body_started.get() {
                std::task::Poll::Ready(())
            } else {
                std::task::Poll::Pending
            }
        })
        .await;
        let sibling = Durability::<String, UnusedError>::new(
            "golem-it",
            "drop-sibling",
            DurableFunctionType::WriteRemote,
            &(),
        )
        .run_infallible_async(|| async move {
            drop(abandoned);
            // A synchronous operation inside this async body uses the installed parent.
            Durability::<String, UnusedError>::new(
                "golem-it",
                "sync-child",
                DurableFunctionType::WriteRemote,
                &(),
            )
            .run_infallible(|| perform_callback("sibling".to_string()))
        })
        .await;
        let completed = Durability::<String, UnusedError>::new(
            "golem-it",
            "completed-body",
            DurableFunctionType::WriteRemote,
            &(),
        )
        .run_infallible_async(|| ReadyWithDrop(DropRequest("ready-drop")))
        .await;
        let after = durable_no_input_generator("fresh").await;
        vec![prepared_result, sibling, completed, after]
    }

    async fn cancel_pending_result(&self, fallible: bool) {
        let operation = async {
            let durability = Durability::<bool, String>::new(
                "golem-it",
                "cancel-result",
                DurableFunctionType::WriteRemote,
                &(),
            );
            if fallible {
                let _ = durability
                    .run_async(|| async { Ok::<_, String>(DropResult(true)) })
                    .await;
            } else {
                let _ = durability
                    .run_infallible_async(|| async { DropResult(true) })
                    .await;
            }
            panic!("finish should remain pending until cancellation");
        };
        let cancel = async {
            let response = crate::raw_wasi_http::send_http_request("/cancel").await;
            crate::raw_wasi_http::read_body(response).await;
        };
        (operation, cancel).race().await;
        perform_callback("after-cancel".to_string());
    }

    fn context_probe(&mut self, mode: String) {
        use golem_rust::IntoTypedSchemaValue;
        if matches!(mode.as_str(), "cross" | "stale" | "ancestor") {
            use golem_rust::retry::{
                CountBoxConfig, NamedRetryPolicy, PolicyNode, PredicateNode, RetryPolicy,
                RetryPredicate, set_retry_policy,
            };
            set_retry_policy(&NamedRetryPolicy {
                name: "context-probe-no-retry".to_string(),
                priority: 0,
                predicate: RetryPredicate {
                    nodes: vec![PredicateNode::PredTrue],
                },
                policy: RetryPolicy {
                    nodes: vec![
                        PolicyNode::CountBox(CountBoxConfig {
                            max_retries: 0,
                            inner: 1,
                        }),
                        PolicyNode::Periodic(1_000_000),
                    ],
                },
            });
        }
        match mode.as_str() {
            "save" => {
                self.saved_context = Some(raw_context::exchange_custom_invocation_context(None))
            }
            "cross" => {
                drop(raw_context::exchange_custom_invocation_context(
                    self.saved_context.take(),
                ));
            }
            "empty" => {
                let empty = raw_context::exchange_custom_invocation_context(None);
                let Some(live) = begin_context_probe("context-probe-empty") else {
                    return;
                };
                let scope = raw_context::exchange_custom_invocation_context(Some(empty));
                perform_callback("empty".to_string());
                drop(raw_context::exchange_custom_invocation_context(Some(scope)));
                perform_callback("restored".to_string());
                let response =
                    golem_rust::encode_typed_schema_value(&().into_typed_schema_value().unwrap())
                        .unwrap();
                raw_context::LiveCustomDurableInvocation::finish(live, response, false);
            }
            "drop" | "stale" | "ancestor" => {
                let Some(live) = begin_context_probe("context-probe") else {
                    return;
                };
                let child = if mode == "ancestor" {
                    begin_context_probe("context-probe-child")
                } else {
                    None
                };
                let snapshot = raw_context::exchange_custom_invocation_context(None);
                if mode != "drop" {
                    drop(live);
                    drop(raw_context::exchange_custom_invocation_context(Some(
                        snapshot,
                    )));
                    drop(child);
                } else {
                    drop(snapshot);
                    perform_callback("snapshot-drop".to_string());
                    let response = golem_rust::encode_typed_schema_value(
                        &().into_typed_schema_value().unwrap(),
                    )
                    .unwrap();
                    raw_context::LiveCustomDurableInvocation::finish(live, response, false);
                }
            }
            _ => panic!("unknown context probe"),
        }
    }
}

fn begin_context_probe(name: &str) -> Option<raw_context::LiveCustomDurableInvocation> {
    use golem_rust::IntoTypedSchemaValue;
    let request =
        golem_rust::encode_typed_schema_value(&().into_typed_schema_value().unwrap()).unwrap();
    match raw_context::begin_custom_durable_invocation(
        name,
        request,
        raw_context::DurableFunctionType::WriteRemote,
    ) {
        raw_context::CustomDurableInvocation::Live(live) => Some(live),
        raw_context::CustomDurableInvocation::Replayed(_) => None,
    }
}

struct DropRequest(&'static str);

impl Drop for DropRequest {
    fn drop(&mut self) {
        perform_callback(self.0.to_string());
    }
}

struct DropResult(bool);

impl Clone for DropResult {
    fn clone(&self) -> Self {
        Self(false)
    }
}

impl From<DropResult> for bool {
    fn from(_: DropResult) -> Self {
        true
    }
}

impl From<bool> for DropResult {
    fn from(_: bool) -> Self {
        Self(false)
    }
}

impl Drop for DropResult {
    fn drop(&mut self) {
        if self.0 {
            log::info!("dropping undelivered custom result");
            perform_callback("result-drop".to_string());
        }
    }
}

struct ReadyWithDrop(DropRequest);

impl std::future::Future for ReadyWithDrop {
    type Output = String;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<String> {
        std::task::Poll::Ready(self.0.0.to_string())
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

async fn durable_promise(promise: &golem_rust::PromiseId) -> String {
    Durability::<String, UnusedError>::new(
        "golem-it",
        "test-promise",
        DurableFunctionType::ReadRemote,
        promise,
    )
    .run_infallible_async(|| async {
        String::from_utf8(golem_rust::await_promise(promise).await).unwrap()
    })
    .await
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
