#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::golem::it_exports::golem_it_api::*;
use golem_rust::bindings::golem::durability::durability::DurableFunctionType;
use golem_rust::durability::Durability;
use golem_rust::value_and_type::type_builder::TypeNodeBuilder;
use golem_rust::value_and_type::{FromValueAndType, IntoValue};
use golem_rust::wasm_rpc::{NodeBuilder, WitValueExtractor};
use golem_rust::{with_persistence_level, PersistenceLevel};
use reqwest::{Client, Method};
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
        builder.record().field("payload").string().finish()
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
        builder.record().field("result").string().finish()
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
        builder.variant().unit_case("unused-error").finish()
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
                .persist_infallible(
                    StructuredInput { payload },
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

bindings::export!(Component with_types_in bindings);
