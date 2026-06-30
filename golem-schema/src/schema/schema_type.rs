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
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_normalized_numeric_restrictions"
        )]
        restrictions: Option<NumericRestrictions>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    S16 {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_normalized_numeric_restrictions"
        )]
        restrictions: Option<NumericRestrictions>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    S32 {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_normalized_numeric_restrictions"
        )]
        restrictions: Option<NumericRestrictions>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    S64 {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_normalized_numeric_restrictions"
        )]
        restrictions: Option<NumericRestrictions>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    U8 {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_normalized_numeric_restrictions"
        )]
        restrictions: Option<NumericRestrictions>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    U16 {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_normalized_numeric_restrictions"
        )]
        restrictions: Option<NumericRestrictions>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    U32 {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_normalized_numeric_restrictions"
        )]
        restrictions: Option<NumericRestrictions>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    U64 {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_normalized_numeric_restrictions"
        )]
        restrictions: Option<NumericRestrictions>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    F32 {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_normalized_numeric_restrictions"
        )]
        restrictions: Option<NumericRestrictions>,
        #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
        metadata: MetadataEnvelope,
    },
    F64 {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_normalized_numeric_restrictions"
        )]
        restrictions: Option<NumericRestrictions>,
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
            | SchemaType::S8 { metadata, .. }
            | SchemaType::S16 { metadata, .. }
            | SchemaType::S32 { metadata, .. }
            | SchemaType::S64 { metadata, .. }
            | SchemaType::U8 { metadata, .. }
            | SchemaType::U16 { metadata, .. }
            | SchemaType::U32 { metadata, .. }
            | SchemaType::U64 { metadata, .. }
            | SchemaType::F32 { metadata, .. }
            | SchemaType::F64 { metadata, .. }
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
            | SchemaType::S8 { metadata, .. }
            | SchemaType::S16 { metadata, .. }
            | SchemaType::S32 { metadata, .. }
            | SchemaType::S64 { metadata, .. }
            | SchemaType::U8 { metadata, .. }
            | SchemaType::U16 { metadata, .. }
            | SchemaType::U32 { metadata, .. }
            | SchemaType::U64 { metadata, .. }
            | SchemaType::F32 { metadata, .. }
            | SchemaType::F64 { metadata, .. }
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

    /// The numeric representation of this node, if it is a numeric type.
    pub fn numeric_repr(&self) -> Option<NumericRepr> {
        match self {
            SchemaType::S8 { .. } => Some(NumericRepr::S8),
            SchemaType::S16 { .. } => Some(NumericRepr::S16),
            SchemaType::S32 { .. } => Some(NumericRepr::S32),
            SchemaType::S64 { .. } => Some(NumericRepr::S64),
            SchemaType::U8 { .. } => Some(NumericRepr::U8),
            SchemaType::U16 { .. } => Some(NumericRepr::U16),
            SchemaType::U32 { .. } => Some(NumericRepr::U32),
            SchemaType::U64 { .. } => Some(NumericRepr::U64),
            SchemaType::F32 { .. } => Some(NumericRepr::F32),
            SchemaType::F64 { .. } => Some(NumericRepr::F64),
            _ => None,
        }
    }

    /// The numeric restrictions of this node, if it is a numeric type with
    /// restrictions set.
    pub fn numeric_restrictions(&self) -> Option<&NumericRestrictions> {
        match self {
            SchemaType::S8 { restrictions, .. }
            | SchemaType::S16 { restrictions, .. }
            | SchemaType::S32 { restrictions, .. }
            | SchemaType::S64 { restrictions, .. }
            | SchemaType::U8 { restrictions, .. }
            | SchemaType::U16 { restrictions, .. }
            | SchemaType::U32 { restrictions, .. }
            | SchemaType::U64 { restrictions, .. }
            | SchemaType::F32 { restrictions, .. }
            | SchemaType::F64 { restrictions, .. } => restrictions.as_ref(),
            _ => None,
        }
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
            restrictions: None,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn s16() -> Self {
        Self::S16 {
            restrictions: None,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn s32() -> Self {
        Self::S32 {
            restrictions: None,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn s64() -> Self {
        Self::S64 {
            restrictions: None,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn u8() -> Self {
        Self::U8 {
            restrictions: None,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn u16() -> Self {
        Self::U16 {
            restrictions: None,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn u32() -> Self {
        Self::U32 {
            restrictions: None,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn u64() -> Self {
        Self::U64 {
            restrictions: None,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn f32() -> Self {
        Self::F32 {
            restrictions: None,
            metadata: MetadataEnvelope::default(),
        }
    }
    pub fn f64() -> Self {
        Self::F64 {
            restrictions: None,
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

// --- Numeric ---

/// A numeric bound usable across every numeric representation.
///
/// `SchemaType` derives `Eq`, and an `i64` mantissa cannot represent
/// `u64::MAX`, so the bound is a three-family sum. Float bounds store canonical
/// IEEE-754 `f64` bits (`NaN`/`inf` rejected at construction, `-0.0` normalized
/// to `+0.0`) so the type stays `Eq`; comparisons always decode the bits back to
/// `f64` and compare numerically, never by bit order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub enum NumericBound {
    Signed(i64),
    Unsigned(u64),
    FloatBits(u64),
}

/// Error returned when constructing a numeric bound from an invalid value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericBoundError {
    /// The float value was `NaN` or infinite.
    NonFinite,
}

impl std::fmt::Display for NumericBoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NumericBoundError::NonFinite => write!(f, "numeric float bound must be finite"),
        }
    }
}

impl std::error::Error for NumericBoundError {}

impl NumericBound {
    /// Construct a float bound from an `f64`, rejecting `NaN`/`inf` and
    /// normalizing `-0.0` to `+0.0`.
    pub fn float(value: f64) -> Result<Self, NumericBoundError> {
        if !value.is_finite() {
            return Err(NumericBoundError::NonFinite);
        }
        // Normalize -0.0 to +0.0 so canonical bits are stable for `Eq`.
        let normalized = value + 0.0;
        Ok(NumericBound::FloatBits(normalized.to_bits()))
    }

    /// The float value of a `FloatBits` bound (decoded from canonical bits).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            NumericBound::FloatBits(bits) => Some(f64::from_bits(*bits)),
            _ => None,
        }
    }
}

/// Inline numeric refinement carried by the numeric `SchemaType` variants.
///
/// `None` on the variant means "unconstrained" — the common, hot-path case, a
/// single tag rather than three carried inner `Option`s. The empty restriction
/// set is never stored as `Some`: smart constructors and decoders collapse it to
/// `None` via [`NumericRestrictions::normalize`], and well-formedness rejects a
/// stored `Some(empty)`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct NumericRestrictions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<NumericBound>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<NumericBound>,
    /// Free-form unit annotation. Schema/help metadata only: numeric
    /// `SchemaValue`s carry no unit, so value validation never checks it;
    /// equivalence/subtyping require the normalized unit to match exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

impl NumericRestrictions {
    /// True when there is nothing to constrain (no bounds and no non-empty unit).
    pub fn is_empty(&self) -> bool {
        self.min.is_none()
            && self.max.is_none()
            && self.unit.as_ref().map(|u| u.is_empty()).unwrap_or(true)
    }

    /// Collapse the empty restriction set to `None`, dropping an empty `unit`
    /// and canonicalizing float bounds (so a decoded `-0.0` becomes `+0.0`).
    /// This enforces the canonicalization invariants that `Some(empty)` is never
    /// constructed and that two restriction sets which differ only by float zero
    /// sign compare equal (`SchemaType` derives `Eq` and feeds byte-equivalence).
    pub fn normalize(mut self) -> Option<Self> {
        self.min = self.min.map(canonicalize_bound);
        self.max = self.max.map(canonicalize_bound);
        if self.unit.as_ref().map(|u| u.is_empty()).unwrap_or(false) {
            self.unit = None;
        }
        if self.is_empty() { None } else { Some(self) }
    }

    /// Repr-aware well-formedness check, shared by every validation entry point
    /// (well-formedness, subtyping, value validation, macro refinement) so no
    /// path assumes a prior well-formedness pass.
    ///
    /// Rejects: a stored empty set (`Some(empty)` must never exist), a bound
    /// whose family does not match the repr, an integer repr carrying a float
    /// bound, a bound that does not fit the repr's range, an `F32` bound that
    /// does not round-trip through `f32`, and `min > max` (compared numerically).
    pub fn validate_for_repr(&self, repr: NumericRepr) -> Result<(), NumericRestrictionError> {
        if self.is_empty() {
            return Err(NumericRestrictionError::EmptyStored);
        }
        if let Some(min) = self.min {
            check_bound_repr(min, repr)?;
        }
        if let Some(max) = self.max {
            check_bound_repr(max, repr)?;
        }
        if let (Some(min), Some(max)) = (self.min, self.max) {
            match numeric_bound_cmp(min, max) {
                Some(std::cmp::Ordering::Greater) => {
                    return Err(NumericRestrictionError::MinGreaterThanMax);
                }
                Some(_) => {}
                None => return Err(NumericRestrictionError::FamilyMismatch),
            }
        }
        Ok(())
    }
}

/// The concrete numeric representation of a numeric `SchemaType` variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericRepr {
    S8,
    S16,
    S32,
    S64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
}

impl NumericRepr {
    /// True for the integer reprs (`S*`/`U*`); false for `F32`/`F64`.
    pub fn is_integer(self) -> bool {
        !matches!(self, NumericRepr::F32 | NumericRepr::F64)
    }

    fn family(self) -> NumericFamily {
        match self {
            NumericRepr::S8 | NumericRepr::S16 | NumericRepr::S32 | NumericRepr::S64 => {
                NumericFamily::Signed
            }
            NumericRepr::U8 | NumericRepr::U16 | NumericRepr::U32 | NumericRepr::U64 => {
                NumericFamily::Unsigned
            }
            NumericRepr::F32 | NumericRepr::F64 => NumericFamily::Float,
        }
    }

    fn signed_range(self) -> Option<(i64, i64)> {
        match self {
            NumericRepr::S8 => Some((i8::MIN as i64, i8::MAX as i64)),
            NumericRepr::S16 => Some((i16::MIN as i64, i16::MAX as i64)),
            NumericRepr::S32 => Some((i32::MIN as i64, i32::MAX as i64)),
            NumericRepr::S64 => Some((i64::MIN, i64::MAX)),
            _ => None,
        }
    }

    fn unsigned_max(self) -> Option<u64> {
        match self {
            NumericRepr::U8 => Some(u8::MAX as u64),
            NumericRepr::U16 => Some(u16::MAX as u64),
            NumericRepr::U32 => Some(u32::MAX as u64),
            NumericRepr::U64 => Some(u64::MAX),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NumericFamily {
    Signed,
    Unsigned,
    Float,
}

impl NumericBound {
    fn family(self) -> NumericFamily {
        match self {
            NumericBound::Signed(_) => NumericFamily::Signed,
            NumericBound::Unsigned(_) => NumericFamily::Unsigned,
            NumericBound::FloatBits(_) => NumericFamily::Float,
        }
    }
}

/// Canonicalize a numeric bound so equal values have equal bits. Currently this
/// only affects float bounds: a `-0.0` (which compares equal to `+0.0`
/// numerically but has different bits) is rewritten to canonical `+0.0` so the
/// `Eq`/byte-equivalence invariants hold for decoded bounds. `NaN`/`inf` bits
/// are left untouched (well-formedness rejects them).
fn canonicalize_bound(b: NumericBound) -> NumericBound {
    match b {
        NumericBound::FloatBits(bits) => {
            if f64::from_bits(bits) == 0.0 {
                NumericBound::FloatBits(0)
            } else {
                b
            }
        }
        other => other,
    }
}

/// serde helper: deserialize an `Option<NumericRestrictions>` field on a numeric
/// `SchemaType` variant, applying the same empty→`None` and float canonicalization
/// normalization the WIT/protobuf decoders use, so JSON is a faithful decode
/// boundary (`restrictions: {}` or `unit: ""` collapse to `None`).
fn deserialize_normalized_numeric_restrictions<'de, D>(
    deserializer: D,
) -> Result<Option<NumericRestrictions>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<NumericRestrictions>::deserialize(deserializer)?;
    Ok(raw.and_then(NumericRestrictions::normalize))
}

/// Numeric comparison of two bounds of the same family. Returns `None` when the
/// families differ (which callers treat as a family mismatch / "not a subtype").
pub fn numeric_bound_cmp(a: NumericBound, b: NumericBound) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (NumericBound::Signed(x), NumericBound::Signed(y)) => Some(x.cmp(&y)),
        (NumericBound::Unsigned(x), NumericBound::Unsigned(y)) => Some(x.cmp(&y)),
        (NumericBound::FloatBits(x), NumericBound::FloatBits(y)) => {
            // Both are finite by construction, so partial_cmp is total here.
            f64::from_bits(x).partial_cmp(&f64::from_bits(y))
        }
        _ => None,
    }
}

fn check_bound_repr(bound: NumericBound, repr: NumericRepr) -> Result<(), NumericRestrictionError> {
    if bound.family() != repr.family() {
        return Err(NumericRestrictionError::FamilyMismatch);
    }
    match bound {
        NumericBound::Signed(v) => {
            let (lo, hi) = repr.signed_range().expect("signed family => signed range");
            if v < lo || v > hi {
                return Err(NumericRestrictionError::BoundOutOfRange);
            }
        }
        NumericBound::Unsigned(v) => {
            let hi = repr
                .unsigned_max()
                .expect("unsigned family => unsigned max");
            if v > hi {
                return Err(NumericRestrictionError::BoundOutOfRange);
            }
        }
        NumericBound::FloatBits(bits) => {
            let v = f64::from_bits(bits);
            if !v.is_finite() {
                return Err(NumericRestrictionError::NonFiniteFloat);
            }
            if repr == NumericRepr::F32 && (v as f32) as f64 != v {
                return Err(NumericRestrictionError::FloatNotRoundTrippable);
            }
        }
    }
    Ok(())
}

/// Reasons a numeric restriction set is not well-formed for a given repr.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericRestrictionError {
    /// An empty restriction set was stored as `Some` (must be `None`).
    EmptyStored,
    /// A bound's family does not match the repr (e.g. signed bound on `U32`).
    FamilyMismatch,
    /// A bound value does not fit the repr's range.
    BoundOutOfRange,
    /// A float bound was `NaN`/infinite.
    NonFiniteFloat,
    /// An `F32` bound does not round-trip through `f32`.
    FloatNotRoundTrippable,
    /// `min` is numerically greater than `max`.
    MinGreaterThanMax,
}

impl std::fmt::Display for NumericRestrictionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            NumericRestrictionError::EmptyStored => {
                "numeric restriction set is empty (must be None)"
            }
            NumericRestrictionError::FamilyMismatch => {
                "numeric bound family does not match the numeric type"
            }
            NumericRestrictionError::BoundOutOfRange => {
                "numeric bound does not fit the numeric type's range"
            }
            NumericRestrictionError::NonFiniteFloat => "numeric float bound must be finite",
            NumericRestrictionError::FloatNotRoundTrippable => {
                "f32 numeric bound does not round-trip through f32"
            }
            NumericRestrictionError::MinGreaterThanMax => {
                "numeric min bound is greater than max bound"
            }
        };
        write!(f, "{msg}")
    }
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
    fn secret_spec_defaults_inner_to_string_when_absent() {
        let decoded: SecretSpec = serde_json::from_value(serde_json::json!({
            "category": "api-key"
        }))
        .expect("SecretSpec JSON without inner should deserialize");

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

#[cfg(test)]
mod numeric_restriction_tests {
    use super::*;
    use test_r::test;

    #[test]
    fn normalize_drops_empty_to_none() {
        assert_eq!(NumericRestrictions::default().normalize(), None);
        assert_eq!(
            NumericRestrictions {
                min: None,
                max: None,
                unit: Some(String::new()),
            }
            .normalize(),
            None
        );
    }

    #[test]
    fn normalize_canonicalizes_negative_zero_float_bound() {
        let neg_zero = NumericBound::FloatBits((-0.0f64).to_bits());
        let r = NumericRestrictions {
            min: Some(neg_zero),
            max: None,
            unit: None,
        }
        .normalize()
        .expect("non-empty restriction");
        assert_eq!(r.min, Some(NumericBound::FloatBits(0)));
        // Canonical +0.0 has bits 0, so it now equals a freshly-constructed +0.0.
        assert_eq!(r.min, Some(NumericBound::float(0.0).unwrap()));
    }

    #[test]
    fn serde_empty_restrictions_object_deserializes_to_none() {
        let json = serde_json::json!({ "kind": "u32", "value": { "restrictions": {} } });
        let ty: SchemaType = serde_json::from_value(json).expect("deserialize");
        assert_eq!(ty.numeric_restrictions(), None);
    }

    #[test]
    fn serde_empty_unit_deserializes_to_none() {
        let json =
            serde_json::json!({ "kind": "u32", "value": { "restrictions": { "unit": "" } } });
        let ty: SchemaType = serde_json::from_value(json).expect("deserialize");
        assert_eq!(ty.numeric_restrictions(), None);
    }

    #[test]
    fn serde_empty_unit_with_bound_drops_only_the_unit() {
        // An empty `unit` is non-canonical and must normalize to `None`, but a
        // present bound keeps the restriction set alive (it is not empty).
        let json = serde_json::json!({
            "kind": "u32",
            "value": {
                "restrictions": {
                    "min": { "kind": "unsigned", "value": 1 },
                    "unit": ""
                }
            }
        });
        let ty: SchemaType = serde_json::from_value(json).expect("deserialize");
        assert_eq!(
            ty.numeric_restrictions(),
            Some(&NumericRestrictions {
                min: Some(NumericBound::Unsigned(1)),
                max: None,
                unit: None,
            })
        );
    }

    #[test]
    fn serde_canonicalizes_negative_zero_float_bound() {
        let neg_zero_bits = (-0.0f64).to_bits();
        let json = serde_json::json!({
            "kind": "f64",
            "value": { "restrictions": { "min": { "kind": "float-bits", "value": neg_zero_bits } } }
        });
        let ty: SchemaType = serde_json::from_value(json).expect("deserialize");
        assert_eq!(
            ty.numeric_restrictions().and_then(|r| r.min),
            Some(NumericBound::FloatBits(0))
        );
    }

    #[test]
    fn serde_present_restrictions_round_trip() {
        let original = SchemaType::U32 {
            restrictions: NumericRestrictions {
                min: Some(NumericBound::Unsigned(1)),
                max: Some(NumericBound::Unsigned(100)),
                unit: Some("items".to_string()),
            }
            .normalize(),
            metadata: MetadataEnvelope::default(),
        };
        let json = serde_json::to_value(&original).expect("serialize");
        let back: SchemaType = serde_json::from_value(json).expect("deserialize");
        assert_eq!(original, back);
    }
}
