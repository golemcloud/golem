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

//! Proptest strategy that produces a `(SchemaType, SchemaValue)` pair
//! where the value structurally matches the type by construction.
//!
//! The pairs feed the round-trip property tests for the JSON value and
//! CLI text renderers; they intentionally avoid distinct paired-graph
//! support because the renderers under test do not exercise refs in
//! this strategy.

use crate::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, FieldDiscriminator, NamedFieldType, PathDirection,
    PathKind, PathSpec, QuantitySpec, QuantityValue, QuotaTokenSpec, ResultSpec, SchemaType,
    SecretSpec, TextRestrictions, UnionBranch, UnionSpec, UrlRestrictions, VariantCaseType,
};
use crate::schema::schema_value::{
    BinaryValuePayload, DurationValuePayload, QuotaTokenValuePayload, ResultValuePayload,
    SchemaValue, SecretValuePayload, TextValuePayload, UnionValuePayload, VariantValuePayload,
};
use chrono::{TimeZone, Utc};
use proptest::collection::vec;
use proptest::prelude::*;

pub fn paired_strategy() -> impl Strategy<Value = (SchemaType, SchemaValue)> {
    leaf_paired().prop_recursive(3, 32, 4, |inner| composite_paired(inner.clone()))
}

fn leaf_paired() -> BoxedStrategy<(SchemaType, SchemaValue)> {
    prop_oneof![
        Just((SchemaType::bool(), SchemaValue::Bool(false))),
        any::<i8>().prop_map(|i| (SchemaType::s8(), SchemaValue::S8(i))),
        any::<i16>().prop_map(|i| (SchemaType::s16(), SchemaValue::S16(i))),
        any::<i32>().prop_map(|i| (SchemaType::s32(), SchemaValue::S32(i))),
        (-1_000_000_000_000_000_000i64..1_000_000_000_000_000_000i64)
            .prop_map(|i| (SchemaType::s64(), SchemaValue::S64(i))),
        any::<u8>().prop_map(|i| (SchemaType::u8(), SchemaValue::U8(i))),
        any::<u16>().prop_map(|i| (SchemaType::u16(), SchemaValue::U16(i))),
        any::<u32>().prop_map(|i| (SchemaType::u32(), SchemaValue::U32(i))),
        (0u64..1_000_000_000_000_000_000u64).prop_map(|u| (SchemaType::u64(), SchemaValue::U64(u))),
        (-1.0e6_f32..1.0e6_f32).prop_map(|f| (SchemaType::f32(), SchemaValue::F32(f))),
        (-1.0e6_f64..1.0e6_f64).prop_map(|f| (SchemaType::f64(), SchemaValue::F64(f))),
        any::<char>().prop_map(|c| (SchemaType::char(), SchemaValue::Char(c))),
        "[ -~]{0,8}".prop_map(|s: String| (SchemaType::string(), SchemaValue::String(s))),
        Just((
            SchemaType::r#enum(vec!["a".to_string(), "b".to_string()]),
            SchemaValue::Enum { case: 0 },
        )),
        Just((
            SchemaType::r#enum(vec!["a".to_string(), "b".to_string()]),
            SchemaValue::Enum { case: 1 },
        )),
        Just((
            SchemaType::flags(vec!["x".to_string(), "y".to_string()]),
            SchemaValue::Flags {
                bits: vec![true, false],
            },
        )),
        "[ -~]{0,8}".prop_map(|s: String| (
            SchemaType::text(TextRestrictions::default()),
            SchemaValue::Text(TextValuePayload {
                text: s,
                language: None,
            }),
        )),
        proptest::collection::vec(any::<u8>(), 0..8).prop_map(|bytes| (
            SchemaType::binary(BinaryRestrictions::default()),
            SchemaValue::Binary(BinaryValuePayload {
                bytes,
                mime_type: None,
            }),
        )),
        "[a-zA-Z][a-zA-Z0-9/._-]{0,8}".prop_map(|p: String| (
            SchemaType::path(PathSpec {
                direction: PathDirection::Input,
                kind: PathKind::Any,
                allowed_mime_types: None,
                allowed_extensions: None,
            }),
            SchemaValue::Path { path: p },
        )),
        Just((
            SchemaType::url(UrlRestrictions::default()),
            SchemaValue::Url {
                url: "https://example.com/".to_string(),
            },
        )),
        // Datetimes are restricted to years 0001..2099 so they survive the
        // canonical RFC 3339 round-trip (the canonical year domain is
        // `0000..=9999`).
        (0i64..4_000_000_000i64).prop_map(|s| (
            SchemaType::datetime(),
            SchemaValue::Datetime {
                value: Utc.timestamp_opt(s, 0).single().unwrap(),
            },
        )),
        any::<i64>().prop_map(|n| (
            SchemaType::duration(),
            SchemaValue::Duration(DurationValuePayload { nanoseconds: n }),
        )),
        (-1000i64..1000i64).prop_map(|m| (
            SchemaType::quantity(QuantitySpec {
                base_unit: "kg".to_string(),
                allowed_suffixes: vec![],
                min: None,
                max: None,
            }),
            SchemaValue::Quantity(QuantityValue {
                mantissa: m,
                scale: 0,
                unit: "kg".to_string(),
            }),
        )),
        Just((
            SchemaType::secret(SecretSpec::default()),
            SchemaValue::Secret(SecretValuePayload {
                secret_id: uuid::Uuid::nil(),
                config_key: None,
                version: 0,
                resolved_at: Utc.timestamp_opt(0, 0).unwrap(),
                category: None,
            }),
        )),
        "[a-z][a-z0-9-]{0,4}".prop_map(|r: String| {
            let resource = if r.is_empty() { "r".to_string() } else { r };
            (
                SchemaType::quota_token(QuotaTokenSpec {
                    resource_name: Some(resource.clone()),
                }),
                SchemaValue::QuotaToken(QuotaTokenValuePayload {
                    environment_id: golem_schema::model::EnvironmentId::new(uuid::Uuid::nil()),
                    resource_name: resource,
                    expected_use: 1,
                    last_credit: 0,
                    last_credit_at: Utc.timestamp_opt(1_700_000_000, 0).single().unwrap(),
                }),
            )
        }),
    ]
    .boxed()
}

fn composite_paired(
    inner: BoxedStrategy<(SchemaType, SchemaValue)>,
) -> BoxedStrategy<(SchemaType, SchemaValue)> {
    prop_oneof![
        // record
        vec(inner.clone(), 0..3).prop_map(|pairs| {
            let mut seen = std::collections::HashSet::<String>::new();
            let mut fields: Vec<NamedFieldType> = Vec::with_capacity(pairs.len());
            let mut values: Vec<SchemaValue> = Vec::with_capacity(pairs.len());
            for (i, (t, v)) in pairs.into_iter().enumerate() {
                let name = format!("f{i}");
                if !seen.insert(name.clone()) {
                    continue;
                }
                fields.push(NamedFieldType {
                    name,
                    body: t,
                    metadata: Default::default(),
                });
                values.push(v);
            }
            (
                SchemaType::record(fields),
                SchemaValue::Record { fields: values },
            )
        }),
        // tuple
        vec(inner.clone(), 0..3).prop_map(|pairs| {
            let (elements, values): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
            (
                SchemaType::tuple(elements),
                SchemaValue::Tuple { elements: values },
            )
        }),
        // list — all elements share the same type, so replicate the head value.
        (inner.clone(), 0u8..3u8).prop_map(|((t, v), n)| {
            let elements: Vec<SchemaValue> = (0..n).map(|_| v.clone()).collect();
            (SchemaType::list(t), SchemaValue::List { elements })
        }),
        // fixed list of length 2
        inner.clone().prop_map(|(t, v)| {
            (
                SchemaType::fixed_list(t, 2),
                SchemaValue::FixedList {
                    elements: vec![v.clone(), v],
                },
            )
        }),
        // option (some) — wrap the inner type in a single-case variant when
        // the inner is itself nullable, so the canonical JSON wire form
        // (`null | inner`) stays bijective and `Option<X>` validation
        // (`§4.19`) accepts the resulting type.
        inner.clone().prop_map(|(t, v)| {
            let (t, v) = wrap_if_nullable(t, v);
            (
                SchemaType::option(t),
                SchemaValue::Option {
                    inner: Some(Box::new(v)),
                },
            )
        }),
        // option (none)
        inner.clone().prop_map(|(t, _v)| {
            let (t, _) = wrap_if_nullable(t, SchemaValue::Bool(false));
            (SchemaType::option(t), SchemaValue::Option { inner: None })
        }),
        // result (ok)
        inner.clone().prop_map(|(t, v)| {
            (
                SchemaType::result(ResultSpec {
                    ok: Some(Box::new(t)),
                    err: None,
                }),
                SchemaValue::Result(ResultValuePayload::Ok {
                    value: Some(Box::new(v)),
                }),
            )
        }),
        // result (err)
        inner.clone().prop_map(|(t, v)| {
            (
                SchemaType::result(ResultSpec {
                    ok: None,
                    err: Some(Box::new(t)),
                }),
                SchemaValue::Result(ResultValuePayload::Err {
                    value: Some(Box::new(v)),
                }),
            )
        }),
        // variant without payload
        Just((
            SchemaType::variant(vec![VariantCaseType {
                name: "only".to_string(),
                payload: None,
                metadata: Default::default(),
            }]),
            SchemaValue::Variant(VariantValuePayload {
                case: 0,
                payload: None,
            }),
        )),
        // variant with payload
        inner.clone().prop_map(|(t, v)| {
            (
                SchemaType::variant(vec![VariantCaseType {
                    name: "only".to_string(),
                    payload: Some(t),
                    metadata: Default::default(),
                }]),
                SchemaValue::Variant(VariantValuePayload {
                    case: 0,
                    payload: Some(Box::new(v)),
                }),
            )
        }),
        // map<string, _>
        (inner.clone(), 0u8..3u8).prop_map(|((vt, vv), n)| {
            let mut seen = std::collections::HashSet::<String>::new();
            let mut entries: Vec<(SchemaValue, SchemaValue)> = Vec::new();
            for i in 0..n {
                let k = format!("k{i}");
                if !seen.insert(k.clone()) {
                    continue;
                }
                entries.push((SchemaValue::String(k), vv.clone()));
            }
            (
                SchemaType::map(SchemaType::string(), vt),
                SchemaValue::Map { entries },
            )
        }),
        // union with prefix discriminator (string body); body satisfies prefix.
        Just((
            SchemaType::union(UnionSpec {
                branches: vec![UnionBranch {
                    tag: "u".to_string(),
                    body: SchemaType::string(),
                    discriminator: DiscriminatorRule::Prefix {
                        prefix: "k1:".to_string(),
                    },
                    metadata: Default::default(),
                }],
            }),
            SchemaValue::Union(UnionValuePayload {
                tag: "u".to_string(),
                body: Box::new(SchemaValue::String("k1:hello".to_string())),
            }),
        )),
        // union with field-equals discriminator (record body)
        Just((
            SchemaType::union(UnionSpec {
                branches: vec![UnionBranch {
                    tag: "t".to_string(),
                    body: SchemaType::record(vec![NamedFieldType {
                        name: "kind".to_string(),
                        body: SchemaType::string(),
                        metadata: Default::default(),
                    }]),
                    discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                        field_name: "kind".to_string(),
                        literal: Some("k1".to_string()),
                    }),
                    metadata: Default::default(),
                }],
            }),
            SchemaValue::Union(UnionValuePayload {
                tag: "t".to_string(),
                body: Box::new(SchemaValue::Record {
                    fields: vec![SchemaValue::String("k1".to_string())],
                }),
            }),
        )),
    ]
    .boxed()
}

/// Whether `ty` is nullable on the canonical JSON wire (see `§4.19`).
fn is_nullable(ty: &SchemaType) -> bool {
    match ty {
        SchemaType::Option { .. } => true,
        SchemaType::Union { spec, .. } => spec.branches.iter().any(|b| is_nullable(&b.body)),
        _ => false,
    }
}

/// If `ty` is nullable, wrap it in a single-case variant so `option<wrap>`
/// is well-formed. The wrapped value/type pair stays structurally
/// equivalent to the input under the variant's single case.
fn wrap_if_nullable(ty: SchemaType, value: SchemaValue) -> (SchemaType, SchemaValue) {
    if is_nullable(&ty) {
        let wrapped_ty = SchemaType::variant(vec![VariantCaseType {
            name: "value".to_string(),
            payload: Some(ty),
            metadata: Default::default(),
        }]);
        let wrapped_value = SchemaValue::Variant(VariantValuePayload {
            case: 0,
            payload: Some(Box::new(value)),
        });
        (wrapped_ty, wrapped_value)
    } else {
        (ty, value)
    }
}
