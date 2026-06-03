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

//! Private re-exports used by code emitted by the `IntoSchema` /
//! `FromSchema` derive macros in the `golem-schema-derive` crate.
//!
//! The runtime trait surface and hand-written impls live in
//! [`crate::schema::conversion`]; this module exists only as a stable
//! mount point under `::golem_common::schema::derive::__private` for
//! generated code.

// =====================================================================
// Private re-exports used by generated code (`#[derive(...)]` output)
// =====================================================================

#[doc(hidden)]
pub mod __private {
    pub use crate::schema::conversion::{
        DecodeError, FromSchema, FromSchemaError, IntoSchema, SchemaBuilder, binary_from_value,
        binary_to_value, default_type_id_from, normalize_type_path, path_from_value, path_to_value,
        secret_from_value, secret_to_value, text_from_value, text_to_value, type_id_with_args,
        url_from_value, url_to_value, value_kind,
    };
    pub use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
    pub use crate::schema::metadata::{MetadataEnvelope, Role, TypeId};
    pub use crate::schema::schema_type::{
        BinaryRestrictions, DiscriminatorRule, FieldDiscriminator, NamedFieldType, PathDirection,
        PathKind, PathSpec, QuantitySpec, QuantityValue, QuotaTokenSpec, ResultSpec, SchemaType,
        SecretSpec, TextRestrictions, UnionBranch, UnionSpec, UrlRestrictions, VariantCaseType,
    };
    pub use crate::schema::schema_value::{
        BinaryValuePayload, DurationValuePayload, QuotaTokenValuePayload, ResultValuePayload,
        SchemaValue, SecretValuePayload, TextValuePayload, UnionValuePayload, VariantValuePayload,
    };
}
