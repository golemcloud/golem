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

use crate::schema::metadata::{MetadataEnvelope, TypeId};
use golem_schema_derive::{FromSchema, IntoSchema};
use serde::{Deserialize, Serialize};

/// One schema type in the recursive in-memory representation.
///
/// Every variant carries a [`MetadataEnvelope`] so docs / aliases / examples /
/// deprecation / role can attach to any node, not only to registered defs or
/// named positions. Use [`SchemaType::metadata`] / [`SchemaType::metadata_mut`]
/// for ergonomic access regardless of variant.
///
/// Recursive positions hold owned, boxed children (rather than indices) so
/// consumers can walk and pattern-match the schema directly without chasing
/// indices into a flat node list. The WIT-shaped flat form is reconstructed
/// on demand by [`super::wit`].
///
/// Closed sum types come in two shapes that differ by how the decoder learns
/// which branch a value belongs to:
///
/// - [`SchemaType::Variant`] is a **carried-tag** sum: the value explicitly
///   carries its case (via [`super::VariantValuePayload::case`]). Zero-
///   inference decoding. Natural mapping for language-level algebraic data
///   types.
/// - [`SchemaType::Union`] is an **inferred-tag** sum: the value does not
///   carry its tag. Each branch declares a [`DiscriminatorRule`] and the
///   decoder picks the branch whose rule matches the raw value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
#[schema(named = "schema-type")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub enum SchemaType {
    /// Reference to a named definition in the enclosing
    /// [`super::SchemaGraph`].
    Ref {
        id: TypeId,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },

    // Primitives
    Bool {
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    S8 {
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    S16 {
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    S32 {
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    S64 {
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    U8 {
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    U16 {
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    U32 {
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    U64 {
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    F32 {
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    F64 {
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    Char {
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    String {
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },

    // Structural composites
    Record {
        fields: Vec<NamedFieldType>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    Variant {
        cases: Vec<VariantCaseType>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    Enum {
        cases: Vec<String>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    Flags {
        flags: Vec<String>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    Tuple {
        elements: Vec<SchemaType>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    List {
        element: Box<SchemaType>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    FixedList {
        element: Box<SchemaType>,
        length: u32,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    Map {
        key: Box<SchemaType>,
        value: Box<SchemaType>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    Option {
        inner: Box<SchemaType>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    Result {
        spec: ResultSpec,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },

    // Rich semantic types
    Text {
        restrictions: TextRestrictions,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    Binary {
        restrictions: BinaryRestrictions,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    Path {
        spec: PathSpec,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    Url {
        restrictions: UrlRestrictions,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    Datetime {
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    Duration {
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    Quantity {
        spec: QuantitySpec,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },

    // Discriminated union (closed, inferred-tag)
    Union {
        spec: UnionSpec,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },

    // Capability nodes
    Secret {
        spec: SecretSpec,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    QuotaToken {
        spec: QuotaTokenSpec,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },

    // WASI P3 stubs (parseable only; no semantics yet).
    Future {
        inner: Option<Box<SchemaType>>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    Stream {
        inner: Option<Box<SchemaType>>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
}

impl SchemaType {
    /// Per-node metadata envelope (docs / aliases / examples / deprecation /
    /// role). Always returns a reference; empty envelopes are the default.
    pub fn metadata(&self) -> &MetadataEnvelope {
        match self {
            SchemaType::Ref { metadata, .. }
            | SchemaType::Bool { metadata }
            | SchemaType::S8 { metadata }
            | SchemaType::S16 { metadata }
            | SchemaType::S32 { metadata }
            | SchemaType::S64 { metadata }
            | SchemaType::U8 { metadata }
            | SchemaType::U16 { metadata }
            | SchemaType::U32 { metadata }
            | SchemaType::U64 { metadata }
            | SchemaType::F32 { metadata }
            | SchemaType::F64 { metadata }
            | SchemaType::Char { metadata }
            | SchemaType::String { metadata }
            | SchemaType::Record { metadata, .. }
            | SchemaType::Variant { metadata, .. }
            | SchemaType::Enum { metadata, .. }
            | SchemaType::Flags { metadata, .. }
            | SchemaType::Tuple { metadata, .. }
            | SchemaType::List { metadata, .. }
            | SchemaType::FixedList { metadata, .. }
            | SchemaType::Map { metadata, .. }
            | SchemaType::Option { metadata, .. }
            | SchemaType::Result { metadata, .. }
            | SchemaType::Text { metadata, .. }
            | SchemaType::Binary { metadata, .. }
            | SchemaType::Path { metadata, .. }
            | SchemaType::Url { metadata, .. }
            | SchemaType::Datetime { metadata }
            | SchemaType::Duration { metadata }
            | SchemaType::Quantity { metadata, .. }
            | SchemaType::Union { metadata, .. }
            | SchemaType::Secret { metadata, .. }
            | SchemaType::QuotaToken { metadata, .. }
            | SchemaType::Future { metadata, .. }
            | SchemaType::Stream { metadata, .. } => metadata,
        }
    }

    /// Mutable access to the per-node metadata envelope.
    pub fn metadata_mut(&mut self) -> &mut MetadataEnvelope {
        match self {
            SchemaType::Ref { metadata, .. }
            | SchemaType::Bool { metadata }
            | SchemaType::S8 { metadata }
            | SchemaType::S16 { metadata }
            | SchemaType::S32 { metadata }
            | SchemaType::S64 { metadata }
            | SchemaType::U8 { metadata }
            | SchemaType::U16 { metadata }
            | SchemaType::U32 { metadata }
            | SchemaType::U64 { metadata }
            | SchemaType::F32 { metadata }
            | SchemaType::F64 { metadata }
            | SchemaType::Char { metadata }
            | SchemaType::String { metadata }
            | SchemaType::Record { metadata, .. }
            | SchemaType::Variant { metadata, .. }
            | SchemaType::Enum { metadata, .. }
            | SchemaType::Flags { metadata, .. }
            | SchemaType::Tuple { metadata, .. }
            | SchemaType::List { metadata, .. }
            | SchemaType::FixedList { metadata, .. }
            | SchemaType::Map { metadata, .. }
            | SchemaType::Option { metadata, .. }
            | SchemaType::Result { metadata, .. }
            | SchemaType::Text { metadata, .. }
            | SchemaType::Binary { metadata, .. }
            | SchemaType::Path { metadata, .. }
            | SchemaType::Url { metadata, .. }
            | SchemaType::Datetime { metadata }
            | SchemaType::Duration { metadata }
            | SchemaType::Quantity { metadata, .. }
            | SchemaType::Union { metadata, .. }
            | SchemaType::Secret { metadata, .. }
            | SchemaType::QuotaToken { metadata, .. }
            | SchemaType::Future { metadata, .. }
            | SchemaType::Stream { metadata, .. } => metadata,
        }
    }

    /// Replace this node's metadata envelope, returning `self` for chaining.
    pub fn with_metadata(mut self, metadata: MetadataEnvelope) -> Self {
        *self.metadata_mut() = metadata;
        self
    }

    // --- Ergonomic constructors ------------------------------------------
    //
    // These default the metadata envelope to empty; callers that need
    // metadata can chain `.with_metadata(env)`.

    pub fn ref_to(id: TypeId) -> Self {
        Self::Ref {
            id,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn bool() -> Self {
        Self::Bool {
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn s8() -> Self {
        Self::S8 {
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn s16() -> Self {
        Self::S16 {
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn s32() -> Self {
        Self::S32 {
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn s64() -> Self {
        Self::S64 {
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn u8() -> Self {
        Self::U8 {
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn u16() -> Self {
        Self::U16 {
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn u32() -> Self {
        Self::U32 {
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn u64() -> Self {
        Self::U64 {
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn f32() -> Self {
        Self::F32 {
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn f64() -> Self {
        Self::F64 {
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn char() -> Self {
        Self::Char {
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn string() -> Self {
        Self::String {
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn datetime() -> Self {
        Self::Datetime {
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn duration() -> Self {
        Self::Duration {
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn record(fields: Vec<NamedFieldType>) -> Self {
        Self::Record {
            fields,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn variant(cases: Vec<VariantCaseType>) -> Self {
        Self::Variant {
            cases,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn r#enum(cases: Vec<String>) -> Self {
        Self::Enum {
            cases,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn flags(flags: Vec<String>) -> Self {
        Self::Flags {
            flags,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn tuple(elements: Vec<SchemaType>) -> Self {
        Self::Tuple {
            elements,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn list(element: SchemaType) -> Self {
        Self::List {
            element: Box::new(element),
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn fixed_list(element: SchemaType, length: u32) -> Self {
        Self::FixedList {
            element: Box::new(element),
            length,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn map(key: SchemaType, value: SchemaType) -> Self {
        Self::Map {
            key: Box::new(key),
            value: Box::new(value),
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn option(inner: SchemaType) -> Self {
        Self::Option {
            inner: Box::new(inner),
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn result(spec: ResultSpec) -> Self {
        Self::Result {
            spec,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn text(restrictions: TextRestrictions) -> Self {
        Self::Text {
            restrictions,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn binary(restrictions: BinaryRestrictions) -> Self {
        Self::Binary {
            restrictions,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn path(spec: PathSpec) -> Self {
        Self::Path {
            spec,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn url(restrictions: UrlRestrictions) -> Self {
        Self::Url {
            restrictions,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn quantity(spec: QuantitySpec) -> Self {
        Self::Quantity {
            spec,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn union(spec: UnionSpec) -> Self {
        Self::Union {
            spec,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn secret(spec: SecretSpec) -> Self {
        Self::Secret {
            spec,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn quota_token(spec: QuotaTokenSpec) -> Self {
        Self::QuotaToken {
            spec,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn future(inner: Option<SchemaType>) -> Self {
        Self::Future {
            inner: inner.map(Box::new),
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn stream(inner: Option<SchemaType>) -> Self {
        Self::Stream {
            inner: inner.map(Box::new),
            metadata: MetadataEnvelope::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct NamedFieldType {
    pub name: String,
    pub body: SchemaType,
    #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
    pub metadata: MetadataEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct VariantCaseType {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<SchemaType>,
    #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
    pub metadata: MetadataEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct ResultSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<Box<SchemaType>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<Box<SchemaType>>,
}

// --- Text / Binary ---

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct TextRestrictions {
    /// Optional set of allowed BCP-47 language codes. `None` = unrestricted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub languages: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct BinaryRestrictions {
    /// Optional set of allowed MIME types. `None` = unrestricted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_types: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_bytes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u32>,
}

// --- Path ---

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub enum PathDirection {
    Input,
    Output,
    InOut,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub enum PathKind {
    File,
    Directory,
    Any,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct PathSpec {
    pub direction: PathDirection,
    pub kind: PathKind,
    /// MIME types allowed for content referenced by this path.
    ///
    /// Not enforced by value validation because [`crate::schema::schema_value::SchemaValue::Path`]
    /// carries only a path string. Enforce this only in protocol/adaptation
    /// layers that have MIME metadata, such as HTTP `Content-Type` or a
    /// content-typed envelope. The validator must not read the filesystem
    /// or sniff content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_mime_types: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_extensions: Option<Vec<String>>,
}

// --- URL ---

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct UrlRestrictions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_schemes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_hosts: Option<Vec<String>>,
}

// --- Quantity ---

/// Fixed-point decimal value with unit: numeric value = `mantissa * 10^(-scale)`.
/// `unit` is a free-form string at the value level; the schema's
/// [`QuantitySpec`] constrains the accepted set.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct QuantityValue {
    pub mantissa: i64,
    pub scale: i32,
    pub unit: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct QuantitySpec {
    /// Canonical base unit (e.g., `"kg"`, `"m"`, `"s"`, `"B"`).
    pub base_unit: String,
    /// Suffixes accepted on input and rendered on output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_suffixes: Vec<String>,
    /// Optional inclusive range, expressed in canonical fixed-point form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<QuantityValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<QuantityValue>,
}

// --- Discriminated union ---

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct UnionSpec {
    pub branches: Vec<UnionBranch>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct UnionBranch {
    /// Logical branch name carried in
    /// [`super::UnionValuePayload::tag`] after the decoder resolves the
    /// branch; used by renderers, codegen, docs.
    pub tag: String,
    /// Branch body type. Must be compatible with the discriminator rule (see
    /// schema-construction validation rules in the design).
    pub body: SchemaType,
    /// Rule the decoder uses to pick this branch from a raw value.
    pub discriminator: DiscriminatorRule,
    #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
    pub metadata: MetadataEnvelope,
}

/// How the decoder identifies that a value belongs to a given union branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "rule", content = "value", rename_all = "kebab-case")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub enum DiscriminatorRule {
    /// String value starts with this prefix.
    Prefix { prefix: String },
    /// String value ends with this suffix.
    Suffix { suffix: String },
    /// String value contains this substring.
    Contains { substring: String },
    /// String value matches this anchored regex.
    Regex { regex: String },
    /// Record-shaped value where the named field is present, and — if
    /// `literal` is set — has the given literal string value.
    FieldEquals(FieldDiscriminator),
    /// Record-shaped value where the named field is absent.
    FieldAbsent {
        #[serde(rename = "fieldName")]
        field_name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct FieldDiscriminator {
    pub field_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub literal: Option<String>,
}

// --- Capability nodes ---

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct SecretSpec {
    /// Revealed payload type carried by this secret handle.
    #[serde(default = "default_secret_inner")]
    pub inner: Box<SchemaType>,
    /// Optional categorisation (e.g., `"api-key"`, `"oauth-token"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

fn default_secret_inner() -> Box<SchemaType> {
    Box::new(SchemaType::string())
}

impl Default for SecretSpec {
    fn default() -> Self {
        Self {
            inner: default_secret_inner(),
            category: None,
        }
    }
}

#[cfg(test)]
mod secret_spec_tests {
    use super::*;
    use test_r::test;

    #[test]
    fn secret_ref_value_round_trip_preserves_opaque_identifier_legacy_serde_secret_spec_defaults_inner()
     {
        let decoded: SecretSpec = serde_json::from_value(serde_json::json!({
            "category": "api-key"
        }))
        .expect("legacy SecretSpec JSON without inner should deserialize");

        assert_eq!(decoded.inner.as_ref(), &SchemaType::string());
        assert_eq!(decoded.category.as_deref(), Some("api-key"));
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct QuotaTokenSpec {
    /// Resource name this token covers (declared in the agent manifest).
    /// `None` = any resource permitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_name: Option<String>,
}
