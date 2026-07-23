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

use crate::model::EnvironmentId;
use crate::schema::schema_type::QuantityValue;
use chrono::{DateTime, Utc};
use golem_schema_derive::{FromSchema, IntoSchema};
use serde::{Deserialize, Serialize};

/// The payload carried by [`SchemaValue::Secret`].
///
/// Like quota tokens, secrets are opaque capabilities. On the host (and in
/// feature-neutral builds) the value is a trusted snapshot that contains only
/// stable identity and metadata; plaintext is never stored here. On a guest it
/// is an opaque, affine, take-once owned handle.
#[cfg(not(all(feature = "guest", not(feature = "host"))))]
pub type SecretVariantValue = SecretValuePayload;

/// The payload carried by [`SchemaValue::Secret`] on a guest: an opaque,
/// affine owned handle. See [`SecretVariantValue`] (host build) for details.
#[cfg(all(feature = "guest", not(feature = "host")))]
pub type SecretVariantValue = crate::schema::wit::GuestSecretHandle;

/// The payload carried by [`SchemaValue::QuotaToken`].
///
/// A quota-token is an opaque, unforgeable capability. The representation
/// differs by build target so that the value can never be inspected or
/// fabricated by a guest:
///
/// - On the host (and in feature-neutral builds) it is the trusted internal
///   snapshot [`QuotaTokenValuePayload`], converted to/from an owned
///   `quota-token` handle by a `QuotaTokenResolver` at the WIT boundary.
/// - On a guest it is an opaque, affine, take-once owned handle
///   ([`crate::schema::wit::GuestQuotaTokenHandle`]) that the guest can only
///   hold and transfer, never read.
#[cfg(not(all(feature = "guest", not(feature = "host"))))]
pub type QuotaTokenVariantValue = QuotaTokenValuePayload;

/// The payload carried by [`SchemaValue::QuotaToken`] on a guest: an opaque,
/// affine owned handle. See [`QuotaTokenVariantValue`] (host build) for details.
#[cfg(all(feature = "guest", not(feature = "host")))]
pub type QuotaTokenVariantValue = crate::schema::wit::GuestQuotaTokenHandle;

/// One node in the recursive in-memory schema-value tree.
///
/// Always travels paired with a [`super::SchemaGraph`] (see
/// [`super::TypedSchemaValue`]). The value tree is structurally driven by the
/// schema: record-value payload order matches the schema's field order,
/// variant-value carries a case index, enum-value carries a case index,
/// union-value carries the discriminator's literal tag. The value side does
/// not redundantly carry field names, case names, or named-ref identifiers —
/// those come from the schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
#[schema(named = "schema-value")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub enum SchemaValue {
    // Primitives
    Bool(bool),
    S8(i8),
    S16(i16),
    S32(i32),
    S64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    Char(char),
    String(String),

    // Structural composites
    Record {
        fields: Vec<SchemaValue>,
    },
    Variant(VariantValuePayload),
    Enum {
        case: u32,
    },
    Flags {
        bits: Vec<bool>,
    },
    Tuple {
        elements: Vec<SchemaValue>,
    },
    List {
        elements: Vec<SchemaValue>,
    },
    FixedList {
        elements: Vec<SchemaValue>,
    },
    Map {
        entries: Vec<(SchemaValue, SchemaValue)>,
    },
    Option {
        inner: Option<Box<SchemaValue>>,
    },
    Result(ResultValuePayload),

    // Rich semantic
    Text(TextValuePayload),
    Binary(BinaryValuePayload),
    Path {
        path: String,
    },
    Url {
        url: String,
    },
    Datetime {
        value: DateTime<Utc>,
    },
    Duration(DurationValuePayload),
    Quantity(QuantityValue),

    // Discriminated union
    Union(UnionValuePayload),

    // Capability nodes
    Secret(SecretVariantValue),
    QuotaToken(QuotaTokenVariantValue),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct VariantValuePayload {
    pub case: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Box<SchemaValue>>,
}

/// Result payload: exactly one of `Ok` / `Err` is set. Each inner option
/// allows `result<_, _>` cases whose ok/err type is unit (no payload).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "tag", rename_all = "kebab-case")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub enum ResultValuePayload {
    Ok { value: Option<Box<SchemaValue>> },
    Err { value: Option<Box<SchemaValue>> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct TextValuePayload {
    pub text: String,
    /// BCP-47 language tag, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct BinaryValuePayload {
    pub bytes: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Signed duration as total nanoseconds.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct DurationValuePayload {
    pub nanoseconds: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct UnionValuePayload {
    /// Tag of the branch the decoder resolved, matching one of the
    /// [`super::UnionBranch::tag`] values. Carried so receivers do not have
    /// to re-run discriminator rules to know which branch was matched;
    /// encoders must ensure it agrees with the body.
    pub tag: String,
    /// Underlying value. Its shape matches the resolved branch's body type
    /// and (by construction) satisfies the branch's discriminator rule.
    pub body: Box<SchemaValue>,
}

/// Capability value: the trusted host snapshot of a secret handle.
///
/// This contains only identity and metadata needed to deterministically
/// resurrect a handle. Plaintext secret material lives in the host resource
/// representation / registry store and is never carried by `SchemaValue`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct SecretValuePayload {
    pub secret_id: uuid::Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_key: Option<Vec<String>>,
    pub version: u64,
    pub resolved_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

/// Capability value: the trusted internal/persistent representation of a
/// quota-token, held only inside `SchemaValue::QuotaToken`. Across a WIT
/// boundary the token travels as an opaque, unforgeable owned handle
/// (`quota-token-handle(own<quota-token>)`); the host converts between this
/// snapshot and a handle through a resolver and the receiver re-acquires a live
/// lease against `(environment_id, resource_name)` on demand. This snapshot is
/// never exposed to or constructible by a guest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct QuotaTokenValuePayload {
    pub environment_id: EnvironmentId,
    pub resource_name: String,
    pub expected_use: u64,
    pub last_credit: i64,
    pub last_credit_at: DateTime<Utc>,
}
