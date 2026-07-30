use golem_rust::durability::{Durability, DurableFunctionType};
use golem_rust::{
    FromSchema, IntoSchema, PersistenceLevel, agent_definition, agent_implementation,
    with_persistence_level,
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
        let durability = Durability::<StructuredResult, UnusedError>::new(
            "golem-it",
            "test-callback",
            DurableFunctionType::WriteRemote,
        );
        if durability.is_live() {
            let result = with_persistence_level(PersistenceLevel::PersistNothing, || {
                perform_callback(payload.clone())
            });
            durability
                .persist_infallible(StructuredInput { payload }, StructuredResult { result })
                .result
        } else {
            durability.replay_infallible::<StructuredResult>().result
        }
    }
}

fn perform_callback(payload: String) -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let authority = format!("localhost:{port}");
    let path = format!("/callback?payload={payload}");
    let (status, body) = raw_http::request(Method::Get, &authority, &path, None, None);
    assert_eq!(status, 200, "callback request failed with status {status}");
    String::from_utf8(body).expect("Failed to read response text")
}
