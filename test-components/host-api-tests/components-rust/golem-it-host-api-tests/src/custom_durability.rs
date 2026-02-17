use golem_rust::bindings::golem::durability::durability::{
    DurableFunctionType, LazyInitializedPollable,
};
use golem_rust::durability::Durability;
use golem_rust::golem_wasm::{NodeBuilder, Pollable, WitValueExtractor};
use golem_rust::value_and_type::type_builder::TypeNodeBuilder;
use golem_rust::value_and_type::{FromValueAndType, IntoValue};
use golem_rust::{agent_definition, agent_implementation, with_persistence_level, PersistenceLevel};
use golem_wasi_http::{Client, IncomingBody, InputStream, Method};
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

#[agent_definition]
pub trait CustomDurability {
    fn new(name: String) -> Self;

    fn callback(&self, payload: String) -> String;

    fn lazy_pollable_init(&mut self);
    fn lazy_pollable_test(&self, n: u32) -> String;
}

pub struct CustomDurabilityImpl {
    _name: String,
    lazy_pollable: Option<LazyInitializedPollable>,
    pollable: Option<Pollable>,
    response: RefCell<Option<golem_wasi_http::Response>>,
    input_stream: RefCell<Option<InputStream>>,
    body: RefCell<Option<IncomingBody>>,
}

#[agent_implementation]
impl CustomDurability for CustomDurabilityImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            lazy_pollable: None,
            pollable: None,
            response: RefCell::new(None),
            input_stream: RefCell::new(None),
            body: RefCell::new(None),
        }
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

    fn lazy_pollable_init(&mut self) {
        let lazy_pollable = LazyInitializedPollable::new();
        let pollable = lazy_pollable.subscribe();
        self.lazy_pollable = Some(lazy_pollable);
        self.pollable = Some(pollable);
    }

    fn lazy_pollable_test(&self, n: u32) -> String {
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
                    let (input_stream, body) = new_response.get_raw_input_stream();
                    let pollable = input_stream.subscribe();
                    self.lazy_pollable
                        .as_ref()
                        .expect("lazy_pollable_init must be called first")
                        .set(unsafe { std::mem::transmute(pollable) });
                    *response = Some(new_response);
                    self.body.replace(Some(body));
                    self.input_stream.replace(Some(input_stream));
                }

                self.pollable
                    .as_ref()
                    .expect("lazy_pollable_init must be called first")
                    .block();
                let buf = self
                    .input_stream
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .read(100)
                    .unwrap();
                String::from_utf8(buf).unwrap()
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
