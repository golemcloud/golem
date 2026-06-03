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

use crate::schema::schema_type::QuantityValue;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One node in the recursive in-memory schema-value tree.
///
/// Always travels paired with a [`super::SchemaGraph`] (see
/// [`super::TypedSchemaValue`]). The value tree is structurally driven by the
/// schema: record-value payload order matches the schema's field order,
/// variant-value carries a case index, enum-value carries a case index,
/// union-value carries the discriminator's literal tag. The value side does
/// not redundantly carry field names, case names, or named-ref identifiers —
/// those come from the schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
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
    Secret(SecretValuePayload),
    QuotaToken(QuotaTokenValuePayload),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VariantValuePayload {
    pub case: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Box<SchemaValue>>,
}

/// Result payload: exactly one of `Ok` / `Err` is set. Each inner option
/// allows `result<_, _>` cases whose ok/err type is unit (no payload).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tag", rename_all = "kebab-case")]
pub enum ResultValuePayload {
    Ok { value: Option<Box<SchemaValue>> },
    Err { value: Option<Box<SchemaValue>> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextValuePayload {
    pub text: String,
    /// BCP-47 language tag, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryValuePayload {
    pub bytes: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Signed duration as total nanoseconds.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurationValuePayload {
    pub nanoseconds: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

/// Capability value: secret transport is **by reference**. The schema side
/// declares the secret; the value side carries an opaque reference that the
/// authority resolves on read. The literal secret material never crosses
/// this carrier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretValuePayload {
    pub secret_ref: String,
}

/// Capability value: quota-token transport is **by snapshot**. The receiver
/// re-acquires a live lease against `(environment_id, resource_name)` on
/// demand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaTokenValuePayload {
    pub environment_id: uuid::Uuid,
    pub resource_name: String,
    pub expected_use: u64,
    pub last_credit: i64,
    pub last_credit_at: DateTime<Utc>,
}
