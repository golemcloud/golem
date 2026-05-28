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
use serde::{Deserialize, Serialize};

/// One schema type in the recursive in-memory representation.
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum SchemaType {
    /// Reference to a named definition in the enclosing
    /// [`super::SchemaGraph`].
    Ref(TypeId),

    // Primitives
    Bool,
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
    Char,
    String,

    // Structural composites
    Record {
        fields: Vec<NamedFieldType>,
    },
    Variant {
        cases: Vec<VariantCaseType>,
    },
    Enum {
        cases: Vec<String>,
    },
    Flags {
        flags: Vec<String>,
    },
    Tuple {
        elements: Vec<SchemaType>,
    },
    List {
        element: Box<SchemaType>,
    },
    FixedList {
        element: Box<SchemaType>,
        length: u32,
    },
    Map {
        key: Box<SchemaType>,
        value: Box<SchemaType>,
    },
    Option {
        inner: Box<SchemaType>,
    },
    Result(ResultSpec),

    // Rich semantic types
    Text(TextRestrictions),
    Binary(BinaryRestrictions),
    Path(PathSpec),
    Url(UrlRestrictions),
    Datetime,
    Duration,
    Quantity(QuantitySpec),

    // Discriminated union (closed, inferred-tag)
    Union(UnionSpec),

    // Capability nodes
    Secret(SecretSpec),
    QuotaToken(QuotaTokenSpec),

    // WASI P3 stubs (parseable only; no semantics yet).
    Future {
        inner: Option<Box<SchemaType>>,
    },
    Stream {
        inner: Option<Box<SchemaType>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedFieldType {
    pub name: String,
    pub body: SchemaType,
    #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
    pub metadata: MetadataEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantCaseType {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<SchemaType>,
    #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
    pub metadata: MetadataEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<Box<SchemaType>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<Box<SchemaType>>,
}

// --- Text / Binary ---

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PathDirection {
    Input,
    Output,
    InOut,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PathKind {
    File,
    Directory,
    Any,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathSpec {
    pub direction: PathDirection,
    pub kind: PathKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_mime_types: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_extensions: Option<Vec<String>>,
}

// --- URL ---

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuantityValue {
    pub mantissa: i64,
    pub scale: i32,
    pub unit: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnionSpec {
    pub branches: Vec<UnionBranch>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "rule", content = "value", rename_all = "kebab-case")]
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
    FieldAbsent { field_name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDiscriminator {
    pub field_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub literal: Option<String>,
}

// --- Capability nodes ---

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretSpec {
    /// Optional categorisation (e.g., `"api-key"`, `"oauth-token"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
    pub metadata: MetadataEnvelope,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaTokenSpec {
    /// Resource name this token covers (declared in the agent manifest).
    /// `None` = any resource permitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_name: Option<String>,
    #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
    pub metadata: MetadataEnvelope,
}
