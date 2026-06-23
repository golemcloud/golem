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

//! Drift-safety tests for `#[derive(PoemSchema)]`.
//!
//! The derive emits a `poem_openapi::types::Type` whose registered
//! `MetaSchema` must mirror the **serde** wire representation, while the actual
//! `ToJSON`/`ParseFromJSON` stay serde-backed. These tests use local types that
//! mirror the exact serde shapes used by the real schema model
//! (`SchemaType`, `InputSchema`, `TypeId`, `MetadataEnvelope`, …) and assert:
//!
//! 1. the **structure** of the registered `MetaSchema` (oneOf shape, tag key,
//!    single-value enum, required-ness, nullability, transparent delegation);
//! 2. that representative serde-serialized values **validate** against the
//!    generated `MetaSchema` (a small subset validator), so the schema can't
//!    silently drift from the wire format.

#![allow(dead_code)]

use golem_schema_derive::PoemSchema;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef, Registry};
use poem_openapi::types::Type;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use test_r::test;

test_r::enable!();

// ----------------------------------------------------------------------
// Representative types mirroring the real schema-model serde shapes.
// ----------------------------------------------------------------------

/// Transparent newtype, mirrors `TypeId(String)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, PoemSchema)]
#[serde(transparent)]
struct TypeIdLike(String);

/// All-optional struct, mirrors `MetadataEnvelope` (every field has a serde
/// default / `skip_serializing_if`, so nothing is required).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, PoemSchema)]
struct MetaLike {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    doc: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    aliases: Vec<String>,
}

/// Plain unit enum with `rename_all`, mirrors `AutoInjectedKind` /
/// `PathDirection`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, PoemSchema)]
#[serde(rename_all = "kebab-case")]
enum AutoKindLike {
    AgentId,
    AgentName,
    SelfReference,
}

/// Adjacently-tagged recursive enum, mirrors `SchemaType`
/// (`tag = "kind", content = "value", rename_all = "kebab-case"`) with a
/// per-node optional `metadata`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, PoemSchema)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
enum SchemaTypeLike {
    Bool {
        // Non-`Option` field made optional purely via `#[serde(default)]`
        // (mirrors `metadata: MetadataEnvelope` with a default). Optional but
        // NOT nullable.
        #[serde(default)]
        metadata: MetaLike,
    },
    List(Box<SchemaTypeLike>),
    Record {
        fields: Vec<NamedFieldTypeLike>,
        #[serde(default)]
        metadata: MetaLike,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, PoemSchema)]
struct NamedFieldTypeLike {
    name: String,
    typ: SchemaTypeLike,
}

/// Adjacently-tagged enum with a unit variant and `tag = "tag"`, mirrors
/// `InputSchema` / `OutputSchema` / `Role`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, PoemSchema)]
#[serde(tag = "tag", content = "value", rename_all = "kebab-case")]
enum InputLike {
    Empty,
    Components(Vec<String>),
    Detailed {
        required: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        optional: Option<String>,
    },
}

/// Adjacently-tagged enum whose newtype variant carries `Option<T>`, to verify
/// the content schema is marked `nullable` (serde serializes the `None` case as
/// `{ "kind": "maybe", "value": null }`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, PoemSchema)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
enum NullableContentLike {
    Maybe(Option<String>),
}

/// Internally-tagged enum, mirrors `ResultValuePayload`
/// (`tag = "tag", rename_all = "kebab-case"`): each variant serializes as an
/// object carrying the tag inline alongside its own fields
/// (`{ "tag": "ok", "value": ... }`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, PoemSchema)]
#[serde(tag = "tag", rename_all = "kebab-case")]
enum ResultLike {
    Ok { value: Option<String> },
    Err { value: Option<String> },
}

/// Internally-tagged enum with a unit variant, to verify a unit branch carries
/// only the tag (`{ "tag": "pending" }`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, PoemSchema)]
#[serde(tag = "tag", rename_all = "kebab-case")]
enum InternalUnitLike {
    Pending,
    Done { code: u32 },
}

/// Struct with a tuple-bearing field, mirrors `SchemaValue::Map`
/// (`entries: Vec<(SchemaValue, SchemaValue)>`): serde serializes the entries
/// as an array of two-element arrays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, PoemSchema)]
struct MapLike {
    entries: Vec<(String, u32)>,
}

// ----------------------------------------------------------------------
// Helpers.
// ----------------------------------------------------------------------

fn register<T: Type>() -> Registry {
    let mut registry = Registry::new();
    T::register(&mut registry);
    registry
}

fn schema<'a>(registry: &'a Registry, name: &str) -> &'a MetaSchema {
    registry
        .schemas
        .get(name)
        .unwrap_or_else(|| panic!("schema `{name}` not registered"))
}

fn inline(r: &MetaSchemaRef) -> &MetaSchema {
    match r {
        MetaSchemaRef::Inline(s) => s,
        MetaSchemaRef::Reference(name) => panic!("expected inline schema, got reference `{name}`"),
    }
}

fn prop<'a>(meta: &'a MetaSchema, name: &str) -> &'a MetaSchemaRef {
    &meta
        .properties
        .iter()
        .find(|(k, _)| *k == name)
        .unwrap_or_else(|| panic!("property `{name}` not found"))
        .1
}

// ----------------------------------------------------------------------
// Structure tests.
// ----------------------------------------------------------------------

#[test]
fn transparent_newtype_delegates_to_inner() {
    // No component is registered for the transparent newtype itself.
    let registry = register::<TypeIdLike>();
    assert!(
        !registry.schemas.contains_key("TypeIdLike"),
        "transparent newtype must not register its own component"
    );

    // Its schema_ref is exactly the inner type's schema_ref (a string).
    let newtype_ref = TypeIdLike::schema_ref();
    let inner_ref = String::schema_ref();
    assert_eq!(inline(&newtype_ref).ty, "string");
    assert_eq!(inline(&inner_ref).ty, "string");
}

#[test]
fn all_optional_struct_has_no_required() {
    let registry = register::<MetaLike>();
    let meta = schema(&registry, "MetaLike");
    assert_eq!(meta.ty, "object");
    assert!(
        meta.required.is_empty(),
        "fields with serde default / skip must not be required, got {:?}",
        meta.required
    );
    // `doc: Option<String>` is nullable; `aliases: Vec<String>` is not.
    assert!(inline(prop(meta, "doc")).nullable);
    let aliases = prop(meta, "aliases");
    assert_eq!(inline(aliases).ty, "array");
    assert!(!inline(aliases).nullable);
}

#[test]
fn unit_enum_is_string_enum_with_rename_all() {
    let registry = register::<AutoKindLike>();
    let meta = schema(&registry, "AutoKindLike");
    assert_eq!(meta.ty, "string");
    let cases: Vec<&str> = meta
        .enum_items
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(cases, vec!["agent-id", "agent-name", "self-reference"]);
}

#[test]
fn adjacent_enum_is_one_of_objects() {
    let registry = register::<SchemaTypeLike>();
    let meta = schema(&registry, "SchemaTypeLike");
    assert_eq!(meta.ty, "object");
    assert_eq!(meta.one_of.len(), 3, "one branch per variant");

    // Bool: a struct variant with only an optional field -> requires
    // ["kind","value"]; the tag is a single-value string enum; `value.metadata`
    // is optional (not required) and not nullable (default, not Option).
    let bool_branch = inline(&meta.one_of[0]);
    assert_eq!(bool_branch.required, vec!["kind", "value"]);
    let kind = inline(prop(bool_branch, "kind"));
    assert_eq!(kind.ty, "string");
    assert_eq!(kind.enum_items, vec![json!("bool")]);
    let value = inline(prop(bool_branch, "value"));
    assert_eq!(value.ty, "object");
    assert!(
        value.required.is_empty(),
        "value.metadata must be optional, got {:?}",
        value.required
    );

    // List: a newtype variant -> `value` references the recursive component.
    let list_branch = inline(&meta.one_of[1]);
    assert_eq!(list_branch.required, vec!["kind", "value"]);
    assert_eq!(
        inline(prop(list_branch, "kind")).enum_items,
        vec![json!("list")]
    );
    assert!(matches!(
        prop(list_branch, "value"),
        MetaSchemaRef::Reference(name) if name == "SchemaTypeLike"
    ));
}

#[test]
fn adjacent_enum_unit_variant_omits_content() {
    let registry = register::<InputLike>();
    let meta = schema(&registry, "InputLike");
    assert_eq!(meta.one_of.len(), 3);

    // Tag key is "tag" (not the default "kind").
    let empty = inline(&meta.one_of[0]);
    assert_eq!(empty.required, vec!["tag"], "unit variant has no content");
    assert_eq!(inline(prop(empty, "tag")).enum_items, vec![json!("empty")]);
    assert!(
        empty.properties.iter().all(|(k, _)| *k == "tag"),
        "unit variant must not carry a `value` property"
    );

    let components = inline(&meta.one_of[1]);
    assert_eq!(components.required, vec!["tag", "value"]);
    assert_eq!(inline(prop(components, "value")).ty, "array");
}

#[test]
fn adjacent_newtype_variant_over_option_is_nullable() {
    let registry = register::<NullableContentLike>();
    let meta = schema(&registry, "NullableContentLike");
    let maybe = inline(&meta.one_of[0]);
    assert_eq!(maybe.required, vec!["kind", "value"]);
    let value = inline(prop(maybe, "value"));
    assert!(
        value.nullable,
        "`Option<T>` variant content must be nullable, got {value:?}"
    );

    // Both the `Some` and `None` serde encodings validate.
    for value in [
        NullableContentLike::Maybe(Some("x".to_string())),
        NullableContentLike::Maybe(None),
    ] {
        let json = serde_json::to_value(&value).unwrap();
        validate(&registry, &NullableContentLike::schema_ref(), &json)
            .unwrap_or_else(|e| panic!("value {json} did not validate: {e}"));
    }
}

#[test]
fn internally_tagged_enum_is_one_of_objects_with_inline_tag() {
    let registry = register::<ResultLike>();
    let meta = schema(&registry, "ResultLike");
    assert_eq!(meta.ty, "object");
    assert_eq!(meta.one_of.len(), 2, "one branch per variant");

    // `Ok { value: Option<String> }` -> object whose tag is inline alongside
    // the variant's own `value` field; the tag is required, `value` is not (it
    // is `Option`), and `value` is nullable.
    let ok = inline(&meta.one_of[0]);
    assert_eq!(ok.required, vec!["tag"], "only the tag is required");
    let tag = inline(prop(ok, "tag"));
    assert_eq!(tag.ty, "string");
    assert_eq!(tag.enum_items, vec![json!("ok")]);
    assert!(
        inline(prop(ok, "value")).nullable,
        "Option field must be nullable"
    );

    // Both serde encodings validate.
    for value in [
        ResultLike::Ok {
            value: Some("x".to_string()),
        },
        ResultLike::Err { value: None },
    ] {
        let json = serde_json::to_value(&value).unwrap();
        validate(&registry, &ResultLike::schema_ref(), &json)
            .unwrap_or_else(|e| panic!("value {json} did not validate: {e}"));
    }
}

#[test]
fn internally_tagged_unit_variant_carries_only_tag() {
    let registry = register::<InternalUnitLike>();
    let meta = schema(&registry, "InternalUnitLike");
    assert_eq!(meta.one_of.len(), 2);

    let pending = inline(&meta.one_of[0]);
    assert_eq!(pending.required, vec!["tag"]);
    assert!(
        pending.properties.iter().all(|(k, _)| *k == "tag"),
        "unit variant must carry only the tag property"
    );
    assert_eq!(inline(prop(pending, "tag")).enum_items, vec![json!("pending")]);

    let done = inline(&meta.one_of[1]);
    assert_eq!(done.required, vec!["tag", "code"]);

    for value in [
        InternalUnitLike::Pending,
        InternalUnitLike::Done { code: 7 },
    ] {
        let json = serde_json::to_value(&value).unwrap();
        validate(&registry, &InternalUnitLike::schema_ref(), &json)
            .unwrap_or_else(|e| panic!("value {json} did not validate: {e}"));
    }
}

#[test]
fn tuple_field_is_fixed_length_array() {
    let registry = register::<MapLike>();
    let meta = schema(&registry, "MapLike");

    // `entries: Vec<(String, u32)>` -> array of two-element arrays.
    let entries = inline(prop(meta, "entries"));
    assert_eq!(entries.ty, "array");
    let entry = inline(entries.items.as_ref().expect("entries has items"));
    assert_eq!(entry.ty, "array");
    assert_eq!(entry.min_items, Some(2));
    assert_eq!(entry.max_items, Some(2));

    let value = MapLike {
        entries: vec![("a".to_string(), 1), ("b".to_string(), 2)],
    };
    let json = serde_json::to_value(&value).unwrap();
    validate(&registry, &MapLike::schema_ref(), &json)
        .unwrap_or_else(|e| panic!("value {json} did not validate: {e}"));
}

// ----------------------------------------------------------------------
// Round-trip tests: serde-serialized values validate against the schema.
// ----------------------------------------------------------------------

#[test]
fn round_trip_values_validate_against_schema() {
    // SchemaTypeLike: adjacent unit-less struct, newtype, recursive, struct.
    let mut registry = register::<SchemaTypeLike>();
    SchemaTypeLike::register(&mut registry);

    let values = vec![
        SchemaTypeLike::Bool {
            metadata: MetaLike::default(),
        },
        SchemaTypeLike::Bool {
            metadata: MetaLike {
                doc: Some("a boolean".to_string()),
                aliases: vec!["flag".to_string()],
            },
        },
        // Recursive: List(Box<String-ish>)
        SchemaTypeLike::List(Box::new(SchemaTypeLike::Bool {
            metadata: MetaLike::default(),
        })),
        SchemaTypeLike::Record {
            fields: vec![NamedFieldTypeLike {
                name: "x".to_string(),
                typ: SchemaTypeLike::List(Box::new(SchemaTypeLike::Bool {
                    metadata: MetaLike::default(),
                })),
            }],
            metadata: MetaLike::default(),
        },
    ];

    for value in values {
        let json = serde_json::to_value(&value).unwrap();
        validate(&registry, &SchemaTypeLike::schema_ref(), &json)
            .unwrap_or_else(|e| panic!("value {json} did not validate: {e}"));
    }

    // InputLike: unit, newtype, struct with optional/omitted field.
    let mut registry = register::<InputLike>();
    InputLike::register(&mut registry);
    let inputs = vec![
        InputLike::Empty,
        InputLike::Components(vec!["a".to_string(), "b".to_string()]),
        InputLike::Detailed {
            required: 7,
            optional: None,
        },
        InputLike::Detailed {
            required: 7,
            optional: Some("y".to_string()),
        },
    ];
    for value in inputs {
        let json = serde_json::to_value(&value).unwrap();
        validate(&registry, &InputLike::schema_ref(), &json)
            .unwrap_or_else(|e| panic!("value {json} did not validate: {e}"));
    }
}

// ----------------------------------------------------------------------
// Minimal JSON-schema-subset validator over poem `MetaSchema`.
// ----------------------------------------------------------------------

fn validate(registry: &Registry, schema: &MetaSchemaRef, value: &Value) -> Result<(), String> {
    match schema {
        MetaSchemaRef::Reference(name) => {
            let meta = registry
                .schemas
                .get(name)
                .ok_or_else(|| format!("unresolved reference `{name}`"))?;
            validate_meta(registry, meta, value)
        }
        MetaSchemaRef::Inline(meta) => validate_meta(registry, meta, value),
    }
}

fn validate_meta(registry: &Registry, meta: &MetaSchema, value: &Value) -> Result<(), String> {
    if value.is_null() && meta.nullable {
        return Ok(());
    }

    if !meta.all_of.is_empty() {
        for branch in &meta.all_of {
            // A nullable-only inline marker carries no real constraint.
            if let MetaSchemaRef::Inline(b) = branch
                && b.ty.is_empty()
                && b.one_of.is_empty()
                && b.all_of.is_empty()
            {
                continue;
            }
            validate(registry, branch, value)?;
        }
        return Ok(());
    }

    if !meta.one_of.is_empty() {
        let matches = meta
            .one_of
            .iter()
            .filter(|branch| validate(registry, branch, value).is_ok())
            .count();
        return if matches == 1 {
            Ok(())
        } else {
            Err(format!(
                "expected exactly one oneOf branch to match, got {matches}"
            ))
        };
    }

    match meta.ty {
        "" => Ok(()),
        "object" => {
            let obj = value
                .as_object()
                .ok_or_else(|| format!("expected object, got {value}"))?;
            for req in &meta.required {
                if !obj.contains_key(*req) {
                    return Err(format!("missing required property `{req}` in {value}"));
                }
            }
            for (key, sub) in &meta.properties {
                if let Some(v) = obj.get(*key) {
                    validate(registry, sub, v).map_err(|e| format!("property `{key}`: {e}"))?;
                }
            }
            Ok(())
        }
        "string" => {
            let s = value
                .as_str()
                .ok_or_else(|| format!("expected string, got {value}"))?;
            if !meta.enum_items.is_empty() && !meta.enum_items.iter().any(|e| e == value) {
                return Err(format!("string `{s}` not in enum {:?}", meta.enum_items));
            }
            Ok(())
        }
        "array" => {
            let arr = value
                .as_array()
                .ok_or_else(|| format!("expected array, got {value}"))?;
            if let Some(items) = &meta.items {
                for (i, v) in arr.iter().enumerate() {
                    validate(registry, items, v).map_err(|e| format!("item[{i}]: {e}"))?;
                }
            }
            Ok(())
        }
        "integer" => value
            .as_i64()
            .map(|_| ())
            .or_else(|| value.as_u64().map(|_| ()))
            .ok_or_else(|| format!("expected integer, got {value}")),
        "number" => value
            .as_f64()
            .map(|_| ())
            .ok_or_else(|| format!("expected number, got {value}")),
        "boolean" => value
            .as_bool()
            .map(|_| ())
            .ok_or_else(|| format!("expected boolean, got {value}")),
        other => Err(format!("unsupported schema type `{other}`")),
    }
}
