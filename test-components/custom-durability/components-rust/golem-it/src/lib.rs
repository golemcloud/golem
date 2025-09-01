#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::golem::it_exports::golem_it_api::*;
use golem_rust::bindings::golem::durability::durability::{
    DurableFunctionType, LazyInitializedPollable,
};
use golem_rust::durability::Durability;
use golem_rust::value_and_type::type_builder::TypeNodeBuilder;
use golem_rust::value_and_type::{FromValueAndType, IntoValue};
use golem_rust::wasm_rpc::{NodeBuilder, Pollable, WitValueExtractor};
use golem_rust::{with_persistence_level, PersistenceLevel};
use reqwest::{Client, InputStream, Method};
use std::cell::RefCell;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone)]
struct StructuredInput {
    pub payload: String,
}

impl IntoValue for StructuredInput {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.record().item().string(&self.payload).finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder
            .record(
                Some("StructuredInput".to_string()),
                Some("golem:it/golem-it-api".to_string()),
            )
            .field("payload")
            .string()
            .finish()
    }
}

impl FromValueAndType for StructuredInput {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        Ok(Self {
            payload: extractor
                .field(0)
                .ok_or_else(|| "Missing field: 'payload'".to_string())?
                .string()
                .ok_or_else(|| "The 'payload' field is not a string".to_string())?
                .to_string(),
        })
    }
}

#[derive(Debug, Clone)]
struct StructuredResult {
    pub result: String,
}

impl IntoValue for StructuredResult {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.record().item().string(&self.result).finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder
            .record(
                Some("StructuredResult".to_string()),
                Some("golem:it/golem-it-api".to_string()),
            )
            .field("result")
            .string()
            .finish()
    }
}

impl FromValueAndType for StructuredResult {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        Ok(Self {
            result: extractor
                .field(0)
                .ok_or_else(|| "Missing field: 'result'".to_string())?
                .string()
                .ok_or_else(|| "The 'result' field is not a string".to_string())?
                .to_string(),
        })
    }
}

#[derive(Debug)]
struct UnusedError;

impl Display for UnusedError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "UnusedError")
    }
}

impl IntoValue for UnusedError {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.variant_unit(0)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder
            .variant(
                Some("UnusedError".to_string()),
                Some("golem:it/golem-it-api".to_string()),
            )
            .unit_case("unused-error")
            .finish()
    }
}

impl FromValueAndType for UnusedError {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let (idx, _inner) = extractor
            .variant()
            .ok_or_else(|| "UnusedError should be variant".to_string())?;
        if idx == 0 {
            Ok(UnusedError)
        } else {
            Err(format!("UnusedError should be variant 0, but got {idx}"))
        }
    }
}

struct Component;

impl Guest for Component {
    type LazyPollableTest = LazyPollableTest;

    fn callback(payload: String) -> String {
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
    Client::new()
        .request(
            Method::GET,
            format!("http://localhost:{port}/callback?payload={payload}"),
        )
        .send()
        .expect("Request failed")
        .text()
        .expect("Failed to read response text")
}

struct LazyPollableTest {
    pub lazy_pollable: LazyInitializedPollable,
    pub pollable: Pollable,
    pub response: RefCell<Option<reqwest::Response>>,
    pub input_stream: RefCell<Option<InputStream>>,
}

impl GuestLazyPollableTest for LazyPollableTest {
    fn new() -> Self {
        let lazy_pollable = LazyInitializedPollable::new();
        let pollable = lazy_pollable.subscribe();
        Self {
            lazy_pollable,
            pollable,
            response: RefCell::new(None),
            input_stream: RefCell::new(None),
        }
    }

    fn test(&self, n: u32) -> String {
        let durability = Durability::<StructuredResult, UnusedError>::new(
            "golem-it",
            "test-callback",
            DurableFunctionType::WriteRemote,
        );
        if durability.is_live() {
            let result = with_persistence_level(PersistenceLevel::PersistNothing, || {
                let mut response = self.response.borrow_mut();
                if response.is_none() {
                    let port = std::env::var("PORT").unwrap_or("9999".to_string());
                    let client = Client::new();
                    let mut new_response = client
                        .request(
                            Method::GET,
                            format!("http://localhost:{port}/fetch?idx={n}"),
                        )
                        .send()
                        .expect("Request failed");
                    let input_stream = new_response.get_raw_input_stream();
                    let pollable = input_stream.subscribe();
                    self.lazy_pollable
                        .set(unsafe { std::mem::transmute(pollable) });
                    *response = Some(new_response);
                    self.input_stream.replace(Some(input_stream));
                }

                self.pollable.block();
                let buf = self
                    .input_stream
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .read(100)
                    .unwrap();
                let result = String::from_utf8(buf).unwrap();
                result
            });

            durability
                .persist_infallible(
                    StructuredInput {
                        payload: n.to_string(),
                    },
                    StructuredResult { result },
                )
                .result
        } else {
            durability.replay_infallible::<StructuredResult>().result
        }
    }
}

bindings::export!(Component with_types_in bindings);
