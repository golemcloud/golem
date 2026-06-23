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

pub mod canonical;
pub mod conversion;
pub mod derive;
pub mod graph;
pub mod metadata;
pub mod multimodal;
#[cfg(feature = "full")]
pub mod protobuf;
pub mod schema_type;
pub mod schema_value;
pub mod unstructured;
pub mod validation;
#[cfg(all(
    any(feature = "guest", feature = "host"),
    not(all(feature = "guest", feature = "host"))
))]
pub mod wit;

#[cfg(any(test, feature = "proptest"))]
pub mod proptest_strategies;

pub use conversion::{
    DecodeError, FromSchema, FromSchemaError, IntoSchema, IntoTypedSchemaValue, MergeError,
    SchemaBuilder, merge_agent_graphs, try_into_schema_graph, try_into_typed_schema_value,
};
#[cfg(feature = "derive")]
pub use golem_schema_derive::{FromSchema, IntoSchema, Schema};
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
