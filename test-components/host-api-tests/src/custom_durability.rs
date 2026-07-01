use golem_rust::durability::{Durability, DurableFunctionType};
use golem_rust::{
    FromSchema, IntoSchema, PersistenceLevel, agent_definition, agent_implementation,
    with_persistence_level,
};
use std::fmt::{Display, Formatter};
use wasi::http::types::Method;

use crate::raw_http;

// TODO(p3): the lazy_pollable_* functionality is temporarily disabled until a p3
// replacement for `lazy-initialized-pollable` is designed (see p3-migration-notes.md).
// The commented-out code below is kept to be restored once that support lands.
// use golem_rust::durability::LazyInitializedPollable;
// use golem_wasi_http::{Client, IncomingBody, InputStream};
// use std::cell::RefCell;

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

    // TODO(p3): restore once lazy-initialized-pollable has a p3 replacement.
    // fn lazy_pollable_init(&mut self);
    // fn lazy_pollable_test(&self, n: u32) -> String;
}

pub struct CustomDurabilityImpl {
    _name: String,
    // TODO(p3): restore once lazy-initialized-pollable has a p3 replacement.
    // lazy_pollable: Option<LazyInitializedPollable>,
    // pollable: Option<golem_rust::wasip2::io::poll::Pollable>,
    // response: RefCell<Option<golem_wasi_http::Response>>,
    // input_stream: RefCell<Option<InputStream>>,
    // body: RefCell<Option<IncomingBody>>,
}

#[agent_implementation]
impl CustomDurability for CustomDurabilityImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            // TODO(p3): restore once lazy-initialized-pollable has a p3 replacement.
            // lazy_pollable: None,
            // pollable: None,
            // response: RefCell::new(None),
            // input_stream: RefCell::new(None),
            // body: RefCell::new(None),
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

    // TODO(p3): restore once lazy-initialized-pollable has a p3 replacement.
    // fn lazy_pollable_init(&mut self) {
    //     let lazy_pollable = LazyInitializedPollable::new();
    //     let pollable = lazy_pollable.subscribe();
    //     self.lazy_pollable = Some(lazy_pollable);
    //     self.pollable = Some(pollable);
    // }
    //
    // fn lazy_pollable_test(&self, n: u32) -> String {
    //     let durability = Durability::<StructuredResult, UnusedError>::new(
    //         "golem-it",
    //         "test-callback",
    //         DurableFunctionType::WriteRemote,
    //     );
    //     if durability.is_live() {
    //         let result = with_persistence_level(PersistenceLevel::PersistNothing, || {
    //             let mut response = self.response.borrow_mut();
    //             if response.is_none() {
    //                 let port = std::env::var("PORT").unwrap_or("9999".to_string());
    //                 let client = Client::new();
    //                 let mut new_response = client
    //                     .request(
    //                         Method::GET,
    //                         format!("http://localhost:{port}/fetch?idx={n}"),
    //                     )
    //                     .send()
    //                     .expect("Request failed");
    //                 let (input_stream, body) = new_response.get_raw_input_stream();
    //                 let pollable = input_stream.subscribe();
    //                 self.lazy_pollable
    //                     .as_ref()
    //                     .expect("lazy_pollable_init must be called first")
    //                     .set(pollable);
    //                 *response = Some(new_response);
    //                 self.body.replace(Some(body));
    //                 self.input_stream.replace(Some(input_stream));
    //             }
    //
    //             self.pollable
    //                 .as_ref()
    //                 .expect("lazy_pollable_init must be called first")
    //                 .block();
    //             let buf = self
    //                 .input_stream
    //                 .borrow()
    //                 .as_ref()
    //                 .unwrap()
    //                 .read(100)
    //                 .unwrap();
    //             String::from_utf8(buf).unwrap()
    //         });
    //
    //         durability
    //             .persist_infallible(
    //                 StructuredInput {
    //                     payload: n.to_string(),
    //                 },
    //                 StructuredResult { result },
    //             )
    //             .result
    //     } else {
    //         durability.replay_infallible::<StructuredResult>().result
    //     }
    // }
}

fn perform_callback(payload: String) -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let authority = format!("localhost:{port}");
    let path = format!("/callback?payload={payload}");
    let (status, body) = raw_http::request(Method::Get, &authority, &path, None, None);
    assert_eq!(status, 200, "callback request failed with status {status}");
    String::from_utf8(body).expect("Failed to read response text")
}
