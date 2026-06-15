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

//! Proptest strategies covering every case of
//! [`SchemaType`](crate::schema::schema_type::SchemaType) and
//! [`SchemaValue`](crate::schema::schema_value::SchemaValue), plus helpers for
//! assembling [`SchemaGraph`](crate::schema::graph::SchemaGraph) and
//! [`TypedSchemaValue`](crate::schema::graph::TypedSchemaValue) instances.
//!
//! The strategies are deliberately structural — they do not try to keep a
//! generated value compatible with a generated type, because the round-trip
//! under test only needs to preserve structure, not enforce
//! type-against-value validity (that is the job of the validator landing in
//! a later step).

use crate::model::EnvironmentId;
use crate::schema::graph::{SchemaGraph, SchemaTypeDef, TypedSchemaValue};
use crate::schema::metadata::{MetadataEnvelope, Role, TypeId};
use crate::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, FieldDiscriminator, NamedFieldType, PathDirection,
    PathKind, PathSpec, QuantitySpec, QuantityValue, QuotaTokenSpec, ResultSpec, SchemaType,
    SecretSpec, TextRestrictions, UnionBranch, UnionSpec, UrlRestrictions, VariantCaseType,
};
use crate::schema::schema_value::{
    BinaryValuePayload, DurationValuePayload, QuotaTokenValuePayload, ResultValuePayload,
    SchemaValue, SecretValuePayload, TextValuePayload, UnionValuePayload, VariantValuePayload,
};
use chrono::{DateTime, TimeZone, Utc};
use proptest::collection::{hash_set, vec};
use proptest::option;
use proptest::prelude::*;
use std::collections::HashSet;

// --- Small leaf strategies ---

fn ident_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,7}".prop_map(|s| s.to_string())
}

fn short_string() -> impl Strategy<Value = String> {
    "[ -~]{0,16}".prop_map(|s| s.to_string())
}

fn finite_f32() -> impl Strategy<Value = f32> {
    any::<f32>().prop_filter("finite", |x| x.is_finite())
}

fn finite_f64() -> impl Strategy<Value = f64> {
    any::<f64>().prop_filter("finite", |x| x.is_finite())
}

/// Datetime within a safe range that survives the WIT (seconds, nanoseconds)
/// representation.
fn datetime_strategy() -> impl Strategy<Value = DateTime<Utc>> {
    (-100_000_000_000i64..100_000_000_000i64, 0u32..1_000_000_000).prop_map(|(s, n)| {
        Utc.timestamp_opt(s, n)
            .single()
            .expect("strategy bounds keep timestamps valid")
    })
}

fn role_strategy() -> impl Strategy<Value = Role> {
    prop_oneof![
        Just(Role::Multimodal),
        ident_strategy().prop_map(Role::Other),
    ]
}

fn metadata_strategy() -> impl Strategy<Value = MetadataEnvelope> {
    (
        option::of(short_string()),
        vec(ident_strategy(), 0..3),
        vec(short_string(), 0..3),
        option::of(short_string()),
        option::of(role_strategy()),
    )
        .prop_map(
            |(doc, aliases, examples, deprecated, role)| MetadataEnvelope {
                doc,
                aliases,
                examples,
                deprecated,
                role,
            },
        )
}

// --- Schema-type strategies ---

fn text_restrictions() -> impl Strategy<Value = TextRestrictions> {
    (
        option::of(vec(ident_strategy(), 0..3)),
        option::of(any::<u32>()),
        option::of(any::<u32>()),
        option::of(short_string()),
    )
        .prop_map(
            |(languages, min_length, max_length, regex)| TextRestrictions {
                languages,
                min_length,
                max_length,
                regex,
            },
        )
}

fn binary_restrictions() -> impl Strategy<Value = BinaryRestrictions> {
    (
        option::of(vec(ident_strategy(), 0..3)),
        option::of(any::<u32>()),
        option::of(any::<u32>()),
    )
        .prop_map(|(mime_types, min_bytes, max_bytes)| BinaryRestrictions {
            mime_types,
            min_bytes,
            max_bytes,
        })
}

fn path_spec() -> impl Strategy<Value = PathSpec> {
    (
        prop_oneof![
            Just(PathDirection::Input),
            Just(PathDirection::Output),
            Just(PathDirection::InOut),
        ],
        prop_oneof![
            Just(PathKind::File),
            Just(PathKind::Directory),
            Just(PathKind::Any),
        ],
        option::of(vec(ident_strategy(), 0..3)),
        option::of(vec(ident_strategy(), 0..3)),
    )
        .prop_map(
            |(direction, kind, allowed_mime_types, allowed_extensions)| PathSpec {
                direction,
                kind,
                allowed_mime_types,
                allowed_extensions,
            },
        )
}

fn url_restrictions() -> impl Strategy<Value = UrlRestrictions> {
    (
        option::of(vec(ident_strategy(), 0..3)),
        option::of(vec(ident_strategy(), 0..3)),
    )
        .prop_map(|(allowed_schemes, allowed_hosts)| UrlRestrictions {
            allowed_schemes,
            allowed_hosts,
        })
}

fn quantity_value_strategy() -> impl Strategy<Value = QuantityValue> {
    (any::<i64>(), any::<i32>(), ident_strategy()).prop_map(|(mantissa, scale, unit)| {
        QuantityValue {
            mantissa,
            scale,
            unit,
        }
    })
}

fn quantity_spec() -> impl Strategy<Value = QuantitySpec> {
    (
        ident_strategy(),
        vec(ident_strategy(), 0..3),
        option::of(quantity_value_strategy()),
        option::of(quantity_value_strategy()),
    )
        .prop_map(|(base_unit, allowed_suffixes, min, max)| QuantitySpec {
            base_unit,
            allowed_suffixes,
            min,
            max,
        })
}

fn discriminator_rule() -> impl Strategy<Value = DiscriminatorRule> {
    prop_oneof![
        short_string().prop_map(|prefix| DiscriminatorRule::Prefix { prefix }),
        short_string().prop_map(|suffix| DiscriminatorRule::Suffix { suffix }),
        short_string().prop_map(|substring| DiscriminatorRule::Contains { substring }),
        short_string().prop_map(|regex| DiscriminatorRule::Regex { regex }),
        (ident_strategy(), option::of(short_string())).prop_map(|(field_name, literal)| {
            DiscriminatorRule::FieldEquals(FieldDiscriminator {
                field_name,
                literal,
            })
        }),
        ident_strategy().prop_map(|field_name| DiscriminatorRule::FieldAbsent { field_name }),
    ]
}

fn secret_spec() -> impl Strategy<Value = SecretSpec> {
    option::of(ident_strategy()).prop_map(|category| SecretSpec { category })
}

fn quota_token_spec() -> impl Strategy<Value = QuotaTokenSpec> {
    option::of(ident_strategy()).prop_map(|resource_name| QuotaTokenSpec { resource_name })
}

/// Generator for one [`SchemaType`] case at a chosen depth budget.
///
/// `def_ids` is the set of `TypeId`s that may be referenced via
/// [`SchemaType::Ref`]; when non-empty, `Ref` is in the choice list so
/// recursive cycles can be exercised.
fn schema_type_strategy(depth: u32, def_ids: Vec<TypeId>) -> impl Strategy<Value = SchemaType> {
    let leaf = leaf_schema_type_strategy(def_ids.clone());
    leaf.prop_recursive(depth, 32, 4, move |inner| {
        composite_schema_type_strategy(inner.clone(), def_ids.clone())
    })
}

fn leaf_schema_type_strategy(def_ids: Vec<TypeId>) -> BoxedStrategy<SchemaType> {
    let mut leaves: Vec<BoxedStrategy<SchemaType>> = vec![
        Just(SchemaType::bool()).boxed(),
        Just(SchemaType::s8()).boxed(),
        Just(SchemaType::s16()).boxed(),
        Just(SchemaType::s32()).boxed(),
        Just(SchemaType::s64()).boxed(),
        Just(SchemaType::u8()).boxed(),
        Just(SchemaType::u16()).boxed(),
        Just(SchemaType::u32()).boxed(),
        Just(SchemaType::u64()).boxed(),
        Just(SchemaType::f32()).boxed(),
        Just(SchemaType::f64()).boxed(),
        Just(SchemaType::char()).boxed(),
        Just(SchemaType::string()).boxed(),
        Just(SchemaType::datetime()).boxed(),
        Just(SchemaType::duration()).boxed(),
        text_restrictions().prop_map(SchemaType::text).boxed(),
        binary_restrictions().prop_map(SchemaType::binary).boxed(),
        path_spec().prop_map(SchemaType::path).boxed(),
        url_restrictions().prop_map(SchemaType::url).boxed(),
        quantity_spec().prop_map(SchemaType::quantity).boxed(),
        secret_spec().prop_map(SchemaType::secret).boxed(),
        quota_token_spec().prop_map(SchemaType::quota_token).boxed(),
        vec(ident_strategy(), 0..4)
            .prop_map(SchemaType::r#enum)
            .boxed(),
        vec(ident_strategy(), 0..4)
            .prop_map(SchemaType::flags)
            .boxed(),
        Just(SchemaType::future(None)).boxed(),
        Just(SchemaType::stream(None)).boxed(),
    ];
    if !def_ids.is_empty() {
        leaves.push(
            proptest::sample::select(def_ids)
                .prop_map(SchemaType::ref_to)
                .boxed(),
        );
    }
    proptest::strategy::Union::new(leaves).boxed()
}

fn composite_schema_type_strategy(
    inner: BoxedStrategy<SchemaType>,
    _def_ids: Vec<TypeId>,
) -> BoxedStrategy<SchemaType> {
    prop_oneof![
        // record
        vec(
            (ident_strategy(), inner.clone(), metadata_strategy()).prop_map(
                |(name, body, metadata)| NamedFieldType {
                    name,
                    body,
                    metadata,
                }
            ),
            0..4
        )
        .prop_map(SchemaType::record),
        // variant
        vec(
            (
                ident_strategy(),
                option::of(inner.clone()),
                metadata_strategy()
            )
                .prop_map(|(name, payload, metadata)| VariantCaseType {
                    name,
                    payload,
                    metadata,
                }),
            1..4
        )
        .prop_map(SchemaType::variant),
        // tuple
        vec(inner.clone(), 0..4).prop_map(SchemaType::tuple),
        // list
        inner.clone().prop_map(SchemaType::list),
        // fixed list
        (inner.clone(), any::<u32>()).prop_map(|(t, length)| SchemaType::fixed_list(t, length)),
        // map
        (inner.clone(), inner.clone()).prop_map(|(k, v)| SchemaType::map(k, v)),
        // option
        inner.clone().prop_map(SchemaType::option),
        // result
        (option::of(inner.clone()), option::of(inner.clone())).prop_map(|(ok, err)| {
            SchemaType::result(ResultSpec {
                ok: ok.map(Box::new),
                err: err.map(Box::new),
            })
        }),
        // union
        vec(
            (
                ident_strategy(),
                inner.clone(),
                discriminator_rule(),
                metadata_strategy()
            )
                .prop_map(|(tag, body, discriminator, metadata)| UnionBranch {
                    tag,
                    body,
                    discriminator,
                    metadata,
                }),
            1..4
        )
        .prop_map(|branches| SchemaType::union(UnionSpec { branches })),
        // future / stream with inner
        inner.clone().prop_map(|t| SchemaType::future(Some(t))),
        inner.prop_map(|t| SchemaType::stream(Some(t))),
    ]
    .boxed()
}

// --- Schema-value strategies ---

pub fn schema_value_strategy() -> impl Strategy<Value = SchemaValue> {
    let leaf = leaf_schema_value_strategy();
    leaf.prop_recursive(4, 32, 4, composite_schema_value_strategy)
}

fn leaf_schema_value_strategy() -> BoxedStrategy<SchemaValue> {
    prop_oneof![
        any::<bool>().prop_map(SchemaValue::Bool),
        any::<i8>().prop_map(SchemaValue::S8),
        any::<i16>().prop_map(SchemaValue::S16),
        any::<i32>().prop_map(SchemaValue::S32),
        any::<i64>().prop_map(SchemaValue::S64),
        any::<u8>().prop_map(SchemaValue::U8),
        any::<u16>().prop_map(SchemaValue::U16),
        any::<u32>().prop_map(SchemaValue::U32),
        any::<u64>().prop_map(SchemaValue::U64),
        finite_f32().prop_map(SchemaValue::F32),
        finite_f64().prop_map(SchemaValue::F64),
        any::<char>().prop_map(SchemaValue::Char),
        short_string().prop_map(SchemaValue::String),
        any::<u32>().prop_map(|case| SchemaValue::Enum { case }),
        vec(any::<bool>(), 0..8).prop_map(|bits| SchemaValue::Flags { bits }),
        short_string().prop_map(|path| SchemaValue::Path { path }),
        short_string().prop_map(|url| SchemaValue::Url { url }),
        datetime_strategy().prop_map(|value| SchemaValue::Datetime { value }),
        any::<i64>().prop_map(|nanoseconds| {
            SchemaValue::Duration(DurationValuePayload { nanoseconds })
        }),
        (any::<i64>(), any::<i32>(), ident_strategy()).prop_map(|(mantissa, scale, unit)| {
            SchemaValue::Quantity(QuantityValue {
                mantissa,
                scale,
                unit,
            })
        },),
        (short_string(), option::of(ident_strategy())).prop_map(|(text, language)| {
            SchemaValue::Text(TextValuePayload { text, language })
        }),
        (vec(any::<u8>(), 0..16), option::of(ident_strategy())).prop_map(|(bytes, mime_type)| {
            SchemaValue::Binary(BinaryValuePayload { bytes, mime_type })
        }),
        short_string()
            .prop_map(|secret_ref| { SchemaValue::Secret(SecretValuePayload { secret_ref }) }),
        quota_token_value_strategy(),
    ]
    .boxed()
}

fn quota_token_value_strategy() -> BoxedStrategy<SchemaValue> {
    (
        any::<u64>(),
        any::<u64>(),
        ident_strategy(),
        any::<u64>(),
        any::<i64>(),
        datetime_strategy(),
    )
        .prop_map(
            |(hi, lo, resource_name, expected_use, last_credit, last_credit_at)| {
                SchemaValue::QuotaToken(QuotaTokenValuePayload {
                    environment_id: EnvironmentId::new(uuid::Uuid::from_u64_pair(hi, lo)),
                    resource_name,
                    expected_use,
                    last_credit,
                    last_credit_at,
                })
            },
        )
        .boxed()
}

fn composite_schema_value_strategy(
    inner: BoxedStrategy<SchemaValue>,
) -> BoxedStrategy<SchemaValue> {
    prop_oneof![
        vec(inner.clone(), 0..4).prop_map(|fields| SchemaValue::Record { fields }),
        (any::<u32>(), option::of(inner.clone())).prop_map(|(case, payload)| {
            SchemaValue::Variant(VariantValuePayload {
                case,
                payload: payload.map(Box::new),
            })
        }),
        vec(inner.clone(), 0..4).prop_map(|elements| SchemaValue::Tuple { elements }),
        vec(inner.clone(), 0..4).prop_map(|elements| SchemaValue::List { elements }),
        vec(inner.clone(), 0..4).prop_map(|elements| SchemaValue::FixedList { elements }),
        vec((inner.clone(), inner.clone()), 0..4).prop_map(|entries| SchemaValue::Map { entries }),
        option::of(inner.clone()).prop_map(|i| SchemaValue::Option {
            inner: i.map(Box::new),
        }),
        prop_oneof![
            option::of(inner.clone()).prop_map(|v| SchemaValue::Result(ResultValuePayload::Ok {
                value: v.map(Box::new),
            })),
            option::of(inner.clone()).prop_map(|v| SchemaValue::Result(ResultValuePayload::Err {
                value: v.map(Box::new),
            })),
        ],
        (ident_strategy(), inner).prop_map(|(tag, body)| {
            SchemaValue::Union(UnionValuePayload {
                tag,
                body: Box::new(body),
            })
        }),
    ]
    .boxed()
}

// --- Graph and TypedSchemaValue ---

pub fn schema_graph_strategy() -> impl Strategy<Value = SchemaGraph> {
    hash_set(ident_strategy(), 0..4).prop_flat_map(|ids_set: HashSet<String>| {
        let def_ids: Vec<TypeId> = ids_set.into_iter().map(TypeId::new).collect();
        let def_ids_for_bodies = def_ids.clone();
        let def_ids_for_root = def_ids.clone();
        let def_ids_for_defs = def_ids.clone();

        let bodies_strategy: BoxedStrategy<Vec<SchemaType>> = if def_ids_for_bodies.is_empty() {
            Just(Vec::<SchemaType>::new()).boxed()
        } else {
            vec(
                schema_type_strategy(3, def_ids_for_bodies.clone()),
                def_ids_for_bodies.len(),
            )
            .boxed()
        };

        (
            bodies_strategy,
            vec(metadata_strategy(), def_ids_for_defs.len()),
            vec(option::of(ident_strategy()), def_ids_for_defs.len()),
            schema_type_strategy(3, def_ids_for_root),
        )
            .prop_map(move |(bodies, metas, names, root)| {
                let defs: Vec<SchemaTypeDef> = def_ids
                    .iter()
                    .cloned()
                    .zip(names)
                    .zip(metas)
                    .zip(bodies)
                    .map(|(((id, name), metadata), body)| SchemaTypeDef {
                        id,
                        name,
                        body: body.with_metadata(metadata),
                    })
                    .collect();
                SchemaGraph { defs, root }
            })
    })
}

pub fn typed_schema_value_strategy() -> impl Strategy<Value = TypedSchemaValue> {
    (schema_graph_strategy(), schema_value_strategy())
        .prop_map(|(graph, value)| TypedSchemaValue::new(graph, value))
}

// --- NaN-tolerant value equality ---

/// Compares two [`SchemaValue`]s for round-trip equality, treating NaN
/// floats as equal to themselves (since `f32::NAN != f32::NAN`).
pub fn schema_values_eq(a: &SchemaValue, b: &SchemaValue) -> bool {
    use SchemaValue::*;
    match (a, b) {
        (F32(x), F32(y)) => bitwise_f32_eq(*x, *y),
        (F64(x), F64(y)) => bitwise_f64_eq(*x, *y),
        (Record { fields: a }, Record { fields: b }) => seq_eq(a, b),
        (Tuple { elements: a }, Tuple { elements: b }) => seq_eq(a, b),
        (List { elements: a }, List { elements: b }) => seq_eq(a, b),
        (FixedList { elements: a }, FixedList { elements: b }) => seq_eq(a, b),
        (Map { entries: a }, Map { entries: b }) => {
            a.len() == b.len()
                && a.iter().zip(b.iter()).all(|((ak, av), (bk, bv))| {
                    schema_values_eq(ak, bk) && schema_values_eq(av, bv)
                })
        }
        (Option { inner: a }, Option { inner: b }) => match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => schema_values_eq(a, b),
            _ => false,
        },
        (Variant(a), Variant(b)) => {
            a.case == b.case
                && match (&a.payload, &b.payload) {
                    (None, None) => true,
                    (Some(x), Some(y)) => schema_values_eq(x, y),
                    _ => false,
                }
        }
        (
            Result(ResultValuePayload::Ok { value: a }),
            Result(ResultValuePayload::Ok { value: b }),
        ) => opt_eq(a.as_deref(), b.as_deref()),
        (
            Result(ResultValuePayload::Err { value: a }),
            Result(ResultValuePayload::Err { value: b }),
        ) => opt_eq(a.as_deref(), b.as_deref()),
        (Union(a), Union(b)) => a.tag == b.tag && schema_values_eq(&a.body, &b.body),
        _ => a == b,
    }
}

fn seq_eq(a: &[SchemaValue], b: &[SchemaValue]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| schema_values_eq(x, y))
}

fn opt_eq(a: Option<&SchemaValue>, b: Option<&SchemaValue>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => schema_values_eq(x, y),
        _ => false,
    }
}

fn bitwise_f32_eq(a: f32, b: f32) -> bool {
    a.to_bits() == b.to_bits()
}

fn bitwise_f64_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits()
}
