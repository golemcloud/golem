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

//! Compatibility layer around [`golem_schema`].
//!
//! The schema/value core lives in `golem-schema` so the Rust SDK and the Golem
//! services can share one native model while selecting guest or host WIT
//! bindings through crate features. `golem-common` keeps only the platform
//! extension modules layered on top of that core.

pub mod agent;
mod common_impls;
#[cfg(feature = "full")]
pub mod protobuf;
pub mod render;
pub mod validation;

#[cfg(any(test, feature = "proptest"))]
pub use golem_schema::schema::proptest_strategies;
#[cfg(feature = "full")]
pub use golem_schema::schema::wit;
pub use golem_schema::schema::{
    canonical, conversion, derive, graph, host_managed, metadata, multimodal, schema_type,
    schema_value, unstructured,
};

#[cfg(test)]
mod tests;

pub use agent::{
    AgentConstructorSchema, AgentDependencySchema, AgentMethodSchema, AgentTypeSchema,
    AutoInjectedKind, FALLBACK_OUTPUT_FIELD_NAME, FieldSource, InputSchema,
    MULTIMODAL_PARTS_FIELD_NAME, NamedField, OutputSchema, ParsedAgentId,
    RegisteredAgentTypeSchema, build_input_record, json_input_schema_value_to_typed_schema_value,
    typed_schema_value_with_projected_defs,
};
pub use conversion::{
    DecodeError, FromSchema, FromSchemaError, IntoSchema, IntoTypedSchemaValue, MergeError,
    SchemaBuilder, merge_agent_graphs, try_into_schema_graph, try_into_typed_schema_value,
};
pub use golem_schema_derive::{FromSchema, IntoSchema};
pub use graph::{SchemaGraph, SchemaTypeDef, TypedSchemaValue};
pub use host_managed::{
    HostManagedKind, RedactedSchemaValue, redact_host_managed_type,
    redact_host_managed_typed_value, redact_host_managed_value, redacted_schema_value_debug,
};
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
