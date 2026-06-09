// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Recursive in-memory mirror of the `golem:core@2.0.0` schema model.
//!
//! The WIT package describes a flat, index-based representation suitable for
//! wire transport. Rust consumers work with a recursive form that mechanically
//! converts to and from the flat form. The conversion lives in [`wit`], which
//! is gated on the `full` feature because it depends on the `wit-bindgen` /
//! `wasmtime` bindings re-exported from `golem-wasm`.

#[cfg(feature = "full")]
pub mod adapters;
pub mod agent;
pub mod canonical;
pub mod conversion;
pub mod derive;
pub mod graph;
pub mod metadata;
#[cfg(feature = "full")]
pub mod protobuf;
pub mod render;
pub mod schema_type;
pub mod schema_value;
pub mod validation;
#[cfg(feature = "full")]
pub mod wit;

/// Proptest strategies for `SchemaType` / `SchemaValue` / `SchemaGraph`,
/// available to this crate's tests and (behind the `proptest` feature) to
/// downstream crates' test code.
#[cfg(any(test, feature = "proptest"))]
pub mod proptest_strategies;

#[cfg(test)]
mod tests;

pub use agent::{
    AgentConstructorSchema, AgentDependencySchema, AgentMethodSchema, AgentTypeSchema,
    AutoInjectedKind, FieldSource, InputSchema, NamedField, OutputSchema, ParsedAgentId,
    RegisteredAgentTypeSchema,
};
pub use conversion::{
    DecodeError, FromSchema, FromSchemaError, IntoSchema, MergeError, SchemaBuilder,
    merge_agent_graphs, try_into_schema_graph,
};
pub use golem_schema_derive::{FromSchema, IntoSchema};
pub use graph::{SchemaGraph, SchemaTypeDef, TypedSchemaValue};
pub use metadata::{MetadataEnvelope, Role, TypeId};
pub use schema_type::{
    BinaryRestrictions, DiscriminatorRule, FieldDiscriminator, NamedFieldType, PathDirection,
    PathKind, PathSpec, QuantitySpec, QuantityValue, QuotaTokenSpec, ResultSpec, SchemaType,
    SecretSpec, TextRestrictions, UnionBranch, UnionSpec, UrlRestrictions, VariantCaseType,
};
pub use schema_value::{
    BinaryValuePayload, DurationValuePayload, QuotaTokenValuePayload, ResultValuePayload,
    SchemaValue, SecretValuePayload, TextValuePayload, UnionValuePayload, VariantValuePayload,
};
