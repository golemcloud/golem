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

//! Renderer that produces a JSON Schema document from a `SchemaGraph`/
//! `SchemaType`.

use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::{MetadataEnvelope, TypeId};
use crate::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, PathSpec, QuantitySpec, QuantityValue, QuotaTokenSpec,
    ResultSpec, SchemaType, SecretSpec, TextRestrictions, UnionSpec, UrlRestrictions,
    VariantCaseType,
};
use serde_json::{Map, Number, Value};

const JSON_SCHEMA_DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";
const MIME_TYPE_PATTERN: &str = "^[A-Za-z0-9!#$&^_.+-]+/[A-Za-z0-9!#$&^_.+-]+$";

/// Render `(graph, ty)` to a JSON Schema document. When `ty` is a
/// `Ref(TypeId)` the document is `{ "$defs": {…}, "$ref": "#/$defs/<id>" }`;
/// otherwise the root schema is emitted inline with `$defs` carrying every
/// named definition from the graph plus any union per-branch synthesised
/// schemas under `<root>__branch__<tag>`.
pub fn to_json_schema(graph: &SchemaGraph, ty: &SchemaType) -> Value {
    let mut root = render_type(graph, ty, true);
    let mut defs = render_defs(graph);
    add_union_branch_defs(graph, ty, &mut defs);
    if !defs.is_empty() {
        if let Some(obj) = root.as_object_mut() {
            obj.insert("$defs".to_string(), Value::Object(defs));
        } else {
            let mut wrapper = Map::new();
            wrapper.insert("$defs".to_string(), Value::Object(defs));
            wrapper.insert("allOf".to_string(), Value::Array(vec![root.clone()]));
            root = Value::Object(wrapper);
        }
    }
    if let Some(obj) = root.as_object_mut() {
        // Insert the JSON Schema draft marker at the top of the produced
        // root schema. OpenAPI removes this; see `super::openapi`.
        let mut with_schema = Map::with_capacity(obj.len() + 1);
        with_schema.insert(
            "$schema".to_string(),
            Value::String(JSON_SCHEMA_DRAFT.to_string()),
        );
        for (k, v) in obj.iter() {
            with_schema.insert(k.clone(), v.clone());
        }
        return Value::Object(with_schema);
    }
    root
}

/// Build a `$defs` object covering every named definition in the graph.
pub(super) fn render_defs(graph: &SchemaGraph) -> Map<String, Value> {
    let mut defs = Map::new();
    for def in &graph.defs {
        // The def's metadata now lives on `def.body` directly; `render_type`
        // already attaches inline-node metadata, so no extra `attach_metadata`
        // call is required here.
        let mut body = render_type(graph, &def.body, false);
        if let Some(name) = &def.name
            && let Some(obj) = body.as_object_mut()
        {
            obj.entry("title").or_insert(Value::String(name.clone()));
        }
        defs.insert(escape_pointer_token(&def.id.0), body);
    }
    defs
}

/// Walk every union under the graph and synthesize per-branch `$defs`
/// entries so discriminator-mapping pointers always resolve.
pub(super) fn add_union_branch_defs(
    graph: &SchemaGraph,
    root_ty: &SchemaType,
    defs: &mut Map<String, Value>,
) {
    let mut emitted = std::collections::HashSet::new();
    collect_union_branch_defs(graph, root_ty, defs, &mut emitted);
    for def in &graph.defs {
        collect_union_branch_defs(graph, &def.body, defs, &mut emitted);
    }
}

fn collect_union_branch_defs(
    graph: &SchemaGraph,
    ty: &SchemaType,
    defs: &mut Map<String, Value>,
    emitted: &mut std::collections::HashSet<String>,
) {
    match ty {
        SchemaType::Union { spec, .. } => {
            for branch in spec.branches.iter() {
                let key = branch_def_key(&branch.tag);
                if emitted.insert(key.clone()) {
                    let mut body = render_type(graph, &branch.body, false);
                    attach_metadata(&mut body, &branch.metadata);
                    if let Some(obj) = body.as_object_mut() {
                        // Constrain the branch schema further with the
                        // discriminator. For record-shaped rules this adds
                        // an extra constraint on the discriminator field;
                        // for string rules it adds a `pattern`/`const`.
                        apply_discriminator_constraint(obj, &branch.discriminator);
                    }
                    defs.insert(key, body);
                }
                collect_union_branch_defs(graph, &branch.body, defs, emitted);
            }
        }
        SchemaType::Record { fields, .. } => {
            for f in fields {
                collect_union_branch_defs(graph, &f.body, defs, emitted);
            }
        }
        SchemaType::Variant { cases, .. } => {
            for case in cases {
                if let Some(p) = &case.payload {
                    collect_union_branch_defs(graph, p, defs, emitted);
                }
            }
        }
        SchemaType::Tuple { elements, .. } => {
            for e in elements {
                collect_union_branch_defs(graph, e, defs, emitted);
            }
        }
        SchemaType::List { element, .. }
        | SchemaType::FixedList { element, .. }
        | SchemaType::Option { inner: element, .. } => {
            collect_union_branch_defs(graph, element, defs, emitted);
        }
        SchemaType::Map { key, value, .. } => {
            collect_union_branch_defs(graph, key, defs, emitted);
            collect_union_branch_defs(graph, value, defs, emitted);
        }
        SchemaType::Result { spec, .. } => {
            if let Some(t) = &spec.ok {
                collect_union_branch_defs(graph, t, defs, emitted);
            }
            if let Some(t) = &spec.err {
                collect_union_branch_defs(graph, t, defs, emitted);
            }
        }
        SchemaType::Future { inner, .. } | SchemaType::Stream { inner, .. } => {
            if let Some(t) = inner {
                collect_union_branch_defs(graph, t, defs, emitted);
            }
        }
        _ => {}
    }
}

fn branch_def_key(tag: &str) -> String {
    format!("union__branch__{}", escape_pointer_token(tag))
}

fn apply_discriminator_constraint(obj: &mut Map<String, Value>, rule: &DiscriminatorRule) {
    match rule {
        DiscriminatorRule::Prefix { prefix } => {
            obj.entry("pattern")
                .or_insert(Value::String(format!("^{}", regex_escape(prefix))));
        }
        DiscriminatorRule::Suffix { suffix } => {
            obj.entry("pattern")
                .or_insert(Value::String(format!("{}$", regex_escape(suffix))));
        }
        DiscriminatorRule::Contains { substring } => {
            obj.entry("pattern")
                .or_insert(Value::String(regex_escape(substring)));
        }
        DiscriminatorRule::Regex { regex } => {
            obj.entry("pattern").or_insert(Value::String(regex.clone()));
        }
        DiscriminatorRule::FieldEquals(disc) => {
            // Constrain the field's value with `const` if a literal is set;
            // otherwise just require the field to be present.
            let mut required = obj
                .get("required")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if !required
                .iter()
                .any(|v| v.as_str() == Some(disc.field_name.as_str()))
            {
                required.push(Value::String(disc.field_name.clone()));
            }
            obj.insert("required".to_string(), Value::Array(required));
            if let Some(lit) = &disc.literal {
                let props = obj
                    .entry("properties")
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .expect("properties is object");
                let field = props
                    .entry(disc.field_name.clone())
                    .or_insert_with(|| obj_inline([("type", Value::String("string".to_string()))]));
                if let Some(field_obj) = field.as_object_mut() {
                    field_obj
                        .entry("const")
                        .or_insert(Value::String(lit.clone()));
                }
            }
        }
        DiscriminatorRule::FieldAbsent { field_name } => {
            // Express absence via `not: { required: [field] }`.
            let not = obj_inline([(
                "required",
                Value::Array(vec![Value::String(field_name.clone())]),
            )]);
            obj.insert("not".to_string(), not);
        }
    }
}

pub(super) fn render_type(graph: &SchemaGraph, ty: &SchemaType, root: bool) -> Value {
    let mut rendered = match ty {
        SchemaType::Ref { id, .. } => obj([("$ref", Value::String(ref_pointer(id, root)))]),

        SchemaType::Bool { .. } => obj([("type", Value::String("boolean".to_string()))]),
        SchemaType::S8 { .. } => integer_schema(i8::MIN as i64, i8::MAX as i64),
        SchemaType::S16 { .. } => integer_schema(i16::MIN as i64, i16::MAX as i64),
        SchemaType::S32 { .. } => integer_schema(i32::MIN as i64, i32::MAX as i64),
        SchemaType::S64 { .. } => integer_schema(i64::MIN, i64::MAX),
        SchemaType::U8 { .. } => integer_schema(0, u8::MAX as i64),
        SchemaType::U16 { .. } => integer_schema(0, u16::MAX as i64),
        SchemaType::U32 { .. } => integer_schema(0, u32::MAX as i64),
        SchemaType::U64 { .. } => unsigned_64_schema(),
        SchemaType::F32 { .. } | SchemaType::F64 { .. } => {
            obj([("type", Value::String("number".to_string()))])
        }
        SchemaType::Char { .. } => obj([
            ("type", Value::String("string".to_string())),
            ("minLength", Value::Number(1.into())),
            ("maxLength", Value::Number(1.into())),
        ]),
        SchemaType::String { .. } => obj([("type", Value::String("string".to_string()))]),

        SchemaType::Record { fields, .. } => {
            let mut props = Map::new();
            let mut required = Vec::with_capacity(fields.len());
            for field in fields {
                let mut field_schema = render_type(graph, &field.body, false);
                attach_metadata(&mut field_schema, &field.metadata);
                props.insert(field.name.clone(), field_schema);
                required.push(Value::String(field.name.clone()));
            }
            obj([
                ("type", Value::String("object".to_string())),
                ("properties", Value::Object(props)),
                ("required", Value::Array(required)),
                ("additionalProperties", Value::Bool(false)),
            ])
        }

        SchemaType::Variant { cases, .. } => Value::Object(variant_schema(graph, cases)),

        SchemaType::Enum { cases, .. } => obj([
            ("type", Value::String("string".to_string())),
            (
                "enum",
                Value::Array(cases.iter().cloned().map(Value::String).collect()),
            ),
        ]),

        SchemaType::Flags { flags, .. } => obj([
            ("type", Value::String("array".to_string())),
            (
                "items",
                obj([
                    ("type", Value::String("string".to_string())),
                    (
                        "enum",
                        Value::Array(flags.iter().cloned().map(Value::String).collect()),
                    ),
                ]),
            ),
            ("uniqueItems", Value::Bool(true)),
        ]),

        SchemaType::Tuple { elements, .. } => {
            if elements.is_empty() {
                // JSON Schema 2020-12 requires `prefixItems` to be a
                // non-empty array, so the empty-tuple shape uses
                // `maxItems`/`minItems` only.
                obj([
                    ("type", Value::String("array".to_string())),
                    ("minItems", Value::Number(0u64.into())),
                    ("maxItems", Value::Number(0u64.into())),
                ])
            } else {
                obj([
                    ("type", Value::String("array".to_string())),
                    (
                        "prefixItems",
                        Value::Array(
                            elements
                                .iter()
                                .map(|e| render_type(graph, e, false))
                                .collect(),
                        ),
                    ),
                    ("items", Value::Bool(false)),
                    ("minItems", Value::Number((elements.len() as u64).into())),
                ])
            }
        }

        SchemaType::List { element, .. } => obj([
            ("type", Value::String("array".to_string())),
            ("items", render_type(graph, element, false)),
        ]),

        SchemaType::FixedList {
            element, length, ..
        } => obj([
            ("type", Value::String("array".to_string())),
            ("items", render_type(graph, element, false)),
            ("minItems", Value::Number((*length).into())),
            ("maxItems", Value::Number((*length).into())),
        ]),

        SchemaType::Map { key, value, .. } => {
            let pair = obj([
                ("type", Value::String("array".to_string())),
                (
                    "prefixItems",
                    Value::Array(vec![
                        render_type(graph, key, false),
                        render_type(graph, value, false),
                    ]),
                ),
                ("items", Value::Bool(false)),
                ("minItems", Value::Number(2.into())),
                ("maxItems", Value::Number(2.into())),
            ]);
            obj([
                ("type", Value::String("array".to_string())),
                ("items", pair),
            ])
        }

        SchemaType::Option { inner, .. } => obj([(
            "oneOf",
            Value::Array(vec![
                obj([("type", Value::String("null".to_string()))]),
                render_type(graph, inner, false),
            ]),
        )]),

        SchemaType::Result { spec, .. } => Value::Object(result_schema(graph, spec)),

        SchemaType::Text {
            restrictions,
            ..
        } => Value::Object(text_schema(restrictions)),
        SchemaType::Binary {
            restrictions,
            ..
        } => Value::Object(binary_schema(restrictions)),
        SchemaType::Path { spec, .. } => Value::Object(path_schema(spec)),
        SchemaType::Url {
            restrictions,
            ..
        } => Value::Object(url_schema(restrictions)),
        SchemaType::Datetime { .. } => obj([
            ("type", Value::String("string".to_string())),
            ("format", Value::String("date-time".to_string())),
        ]),
        SchemaType::Duration { .. } => obj([
            ("type", Value::String("string".to_string())),
            ("format", Value::String("duration".to_string())),
        ]),
        SchemaType::Quantity { spec, .. } => Value::Object(quantity_schema(spec)),

        SchemaType::Union { spec, .. } => Value::Object(union_schema(graph, spec)),

        SchemaType::Secret { spec, .. } => Value::Object(secret_schema(spec)),
        SchemaType::QuotaToken { spec, .. } => Value::Object(quota_token_schema(spec)),

        SchemaType::Future { .. } | SchemaType::Stream { .. } => obj([
            ("type", Value::String("null".to_string())),
            (
                "description",
                Value::String("WASI P3 placeholder".to_string()),
            ),
        ]),
    };

    // Per-node metadata: attach docs / examples / deprecated for every
    // SchemaType node so inline-typed positions (record fields, list
    // elements, etc.) propagate their metadata into the generated JSON
    // Schema, not only named definitions.
    attach_metadata(&mut rendered, ty.metadata());
    rendered
}

fn ref_pointer(id: &TypeId, _root: bool) -> String {
    format!("#/$defs/{}", escape_pointer_token(&id.0))
}

fn integer_schema(min: i64, max: i64) -> Value {
    obj([
        ("type", Value::String("integer".to_string())),
        ("minimum", Value::Number(Number::from(min))),
        ("maximum", Value::Number(Number::from(max))),
    ])
}

fn unsigned_64_schema() -> Value {
    obj([
        ("type", Value::String("integer".to_string())),
        ("minimum", Value::Number(Number::from(0u64))),
        ("maximum", Value::Number(Number::from(u64::MAX))),
    ])
}

fn variant_schema(graph: &SchemaGraph, cases: &[VariantCaseType]) -> Map<String, Value> {
    let one_of: Vec<Value> = cases
        .iter()
        .map(|case| match &case.payload {
            None => obj([("const", Value::String(case.name.clone()))]),
            Some(payload_ty) => {
                let mut props = Map::new();
                props.insert(case.name.clone(), render_type(graph, payload_ty, false));
                obj([
                    ("type", Value::String("object".to_string())),
                    ("properties", Value::Object(props)),
                    (
                        "required",
                        Value::Array(vec![Value::String(case.name.clone())]),
                    ),
                    ("additionalProperties", Value::Bool(false)),
                ])
            }
        })
        .collect();
    let mut out = Map::new();
    out.insert("oneOf".to_string(), Value::Array(one_of));
    out
}

fn result_schema(graph: &SchemaGraph, spec: &ResultSpec) -> Map<String, Value> {
    let ok_inner = spec
        .ok
        .as_deref()
        .map(|t| render_type(graph, t, false))
        .unwrap_or_else(|| obj([("type", Value::String("null".to_string()))]));
    let err_inner = spec
        .err
        .as_deref()
        .map(|t| render_type(graph, t, false))
        .unwrap_or_else(|| obj([("type", Value::String("null".to_string()))]));
    let one_of = vec![
        obj([
            ("type", Value::String("object".to_string())),
            (
                "properties",
                Value::Object({
                    let mut m = Map::new();
                    m.insert("ok".to_string(), ok_inner);
                    m
                }),
            ),
            ("required", Value::Array(vec![Value::String("ok".into())])),
            ("additionalProperties", Value::Bool(false)),
        ]),
        obj([
            ("type", Value::String("object".to_string())),
            (
                "properties",
                Value::Object({
                    let mut m = Map::new();
                    m.insert("err".to_string(), err_inner);
                    m
                }),
            ),
            ("required", Value::Array(vec![Value::String("err".into())])),
            ("additionalProperties", Value::Bool(false)),
        ]),
    ];
    let mut out = Map::new();
    out.insert("oneOf".to_string(), Value::Array(one_of));
    out
}

fn text_schema(restrictions: &TextRestrictions) -> Map<String, Value> {
    // Canonical Text JSON shape: `{ text: string, language?: string }` with
    // length / pattern constraints lifted into the `text` field.
    let mut text_field = Map::new();
    text_field.insert("type".to_string(), Value::String("string".to_string()));
    if let Some(min) = restrictions.min_length {
        text_field.insert("minLength".to_string(), Value::Number(min.into()));
    }
    if let Some(max) = restrictions.max_length {
        text_field.insert("maxLength".to_string(), Value::Number(max.into()));
    }
    if let Some(regex) = &restrictions.regex {
        text_field.insert("pattern".to_string(), Value::String(regex.clone()));
    }
    let mut properties = Map::new();
    properties.insert("text".to_string(), Value::Object(text_field));
    properties.insert(
        "language".to_string(),
        obj([("type", Value::String("string".to_string()))]),
    );
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("object".to_string()));
    m.insert("properties".to_string(), Value::Object(properties));
    m.insert(
        "required".to_string(),
        Value::Array(vec![Value::String("text".to_string())]),
    );
    m.insert("additionalProperties".to_string(), Value::Bool(false));
    if let Some(langs) = &restrictions.languages {
        m.insert(
            "description".to_string(),
            Value::String(format!("Allowed languages: {}", langs.join(", "))),
        );
    }
    m
}

fn binary_schema(restrictions: &BinaryRestrictions) -> Map<String, Value> {
    // Canonical Binary JSON shape: `{ bytes: base64url-string, mime_type?: string }`.
    let mut bytes_field = Map::new();
    bytes_field.insert("type".to_string(), Value::String("string".to_string()));
    bytes_field.insert(
        "contentEncoding".to_string(),
        Value::String("base64url".to_string()),
    );
    if let Some(min) = restrictions.min_bytes {
        bytes_field.insert("minLength".to_string(), Value::Number(min.into()));
    }
    if let Some(max) = restrictions.max_bytes {
        bytes_field.insert("maxLength".to_string(), Value::Number(max.into()));
    }
    let mut mime_field = Map::new();
    mime_field.insert("type".to_string(), Value::String("string".to_string()));
    mime_field.insert(
        "pattern".to_string(),
        Value::String(MIME_TYPE_PATTERN.to_string()),
    );
    let mut properties = Map::new();
    properties.insert("bytes".to_string(), Value::Object(bytes_field));
    properties.insert("mime_type".to_string(), Value::Object(mime_field));
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("object".to_string()));
    m.insert("properties".to_string(), Value::Object(properties));
    m.insert(
        "required".to_string(),
        Value::Array(vec![Value::String("bytes".to_string())]),
    );
    m.insert("additionalProperties".to_string(), Value::Bool(false));
    if let Some(mimes) = &restrictions.mime_types {
        m.insert(
            "description".to_string(),
            Value::String(format!("Allowed MIME types: {}", mimes.join(", "))),
        );
    }
    m
}

fn path_schema(spec: &PathSpec) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("string".to_string()));
    let kind = match spec.kind {
        crate::schema::schema_type::PathKind::File => "file",
        crate::schema::schema_type::PathKind::Directory => "directory",
        crate::schema::schema_type::PathKind::Any => "any",
    };
    let direction = match spec.direction {
        crate::schema::schema_type::PathDirection::Input => "input",
        crate::schema::schema_type::PathDirection::Output => "output",
        crate::schema::schema_type::PathDirection::InOut => "inout",
    };
    m.insert(
        "title".to_string(),
        Value::String(format!("{direction} {kind} path")),
    );
    let mut description = Vec::new();
    if let Some(exts) = &spec.allowed_extensions {
        description.push(format!("Allowed extensions: {}", exts.join(", ")));
    }
    if let Some(mimes) = &spec.allowed_mime_types {
        description.push(format!("Allowed MIME types: {}", mimes.join(", ")));
    }
    if !description.is_empty() {
        m.insert(
            "description".to_string(),
            Value::String(description.join("; ")),
        );
    }
    m
}

fn url_schema(restrictions: &UrlRestrictions) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("string".to_string()));
    m.insert("title".to_string(), Value::String("URL".to_string()));
    let mut description = Vec::new();
    if let Some(schemes) = &restrictions.allowed_schemes {
        description.push(format!("Allowed schemes: {}", schemes.join(", ")));
    }
    if let Some(hosts) = &restrictions.allowed_hosts {
        description.push(format!("Allowed hosts: {}", hosts.join(", ")));
    }
    if !description.is_empty() {
        m.insert(
            "description".to_string(),
            Value::String(description.join("; ")),
        );
    }
    m
}

fn quantity_schema(spec: &QuantitySpec) -> Map<String, Value> {
    let mut props = Map::new();
    props.insert(
        "mantissa".to_string(),
        obj([("type", Value::String("integer".to_string()))]),
    );
    props.insert(
        "scale".to_string(),
        obj([("type", Value::String("integer".to_string()))]),
    );
    props.insert(
        "unit".to_string(),
        obj([("type", Value::String("string".to_string()))]),
    );
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("object".to_string()));
    m.insert("properties".to_string(), Value::Object(props));
    m.insert(
        "required".to_string(),
        Value::Array(vec![
            Value::String("mantissa".to_string()),
            Value::String("scale".to_string()),
            Value::String("unit".to_string()),
        ]),
    );
    m.insert("additionalProperties".to_string(), Value::Bool(false));
    m.insert(
        "title".to_string(),
        Value::String(format!("Quantity ({})", spec.base_unit)),
    );
    let mut description = Vec::new();
    if let Some(min) = &spec.min {
        description.push(format!("min: {}", render_quantity(min)));
    }
    if let Some(max) = &spec.max {
        description.push(format!("max: {}", render_quantity(max)));
    }
    if !description.is_empty() {
        m.insert(
            "description".to_string(),
            Value::String(description.join("; ")),
        );
    }
    m
}

fn render_quantity(q: &QuantityValue) -> String {
    format!("{}e-{} {}", q.mantissa, q.scale, q.unit)
}

fn secret_schema(_spec: &SecretSpec) -> Map<String, Value> {
    // Canonical Secret JSON shape: `{ secret_ref: string-non-empty }`.
    let mut secret_ref = Map::new();
    secret_ref.insert("type".to_string(), Value::String("string".to_string()));
    secret_ref.insert("minLength".to_string(), Value::Number(1.into()));
    let mut properties = Map::new();
    properties.insert("secret_ref".to_string(), Value::Object(secret_ref));
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("object".to_string()));
    m.insert("properties".to_string(), Value::Object(properties));
    m.insert(
        "required".to_string(),
        Value::Array(vec![Value::String("secret_ref".to_string())]),
    );
    m.insert("additionalProperties".to_string(), Value::Bool(false));
    m
}

fn quota_token_schema(_spec: &QuotaTokenSpec) -> Map<String, Value> {
    // Canonical QuotaToken JSON shape: see canonical/quota_token.rs.
    let env_id = obj_inline([
        ("type", Value::String("string".to_string())),
        ("format", Value::String("uuid".to_string())),
    ]);
    let resource_name = obj_inline([("type", Value::String("string".to_string()))]);
    let expected_use = obj_inline([(
        "oneOf",
        Value::Array(vec![
            obj_inline([
                ("type", Value::String("string".to_string())),
                ("pattern", Value::String("^[0-9]+$".to_string())),
            ]),
            obj_inline([
                ("type", Value::String("integer".to_string())),
                ("minimum", Value::Number(0u64.into())),
            ]),
        ]),
    )]);
    let last_credit = obj_inline([(
        "oneOf",
        Value::Array(vec![
            obj_inline([
                ("type", Value::String("string".to_string())),
                ("pattern", Value::String("^-?[0-9]+$".to_string())),
            ]),
            obj_inline([("type", Value::String("integer".to_string()))]),
        ]),
    )]);
    let last_credit_at = obj_inline([
        ("type", Value::String("string".to_string())),
        ("format", Value::String("date-time".to_string())),
    ]);
    let mut properties = Map::new();
    properties.insert("environment_id".to_string(), env_id);
    properties.insert("resource_name".to_string(), resource_name);
    properties.insert("expected_use".to_string(), expected_use);
    properties.insert("last_credit".to_string(), last_credit);
    properties.insert("last_credit_at".to_string(), last_credit_at);
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("object".to_string()));
    m.insert("properties".to_string(), Value::Object(properties));
    m.insert(
        "required".to_string(),
        Value::Array(vec![
            Value::String("environment_id".to_string()),
            Value::String("resource_name".to_string()),
            Value::String("expected_use".to_string()),
            Value::String("last_credit".to_string()),
            Value::String("last_credit_at".to_string()),
        ]),
    );
    m.insert("additionalProperties".to_string(), Value::Bool(false));
    m
}

fn union_schema(graph: &SchemaGraph, spec: &UnionSpec) -> Map<String, Value> {
    // Each branch gets a per-branch reference into `$defs` (synthesised
    // by `add_union_branch_defs`), so the `oneOf` and any discriminator
    // mapping resolves against schemas the renderer actually emits.
    let one_of: Vec<Value> = spec
        .branches
        .iter()
        .map(|b| {
            obj([(
                "$ref",
                Value::String(format!("#/$defs/{}", branch_def_key(&b.tag))),
            )])
        })
        .collect();
    let mut m = Map::new();
    m.insert("oneOf".to_string(), Value::Array(one_of));
    if let Some(disc_field) = openapi_discriminator(spec) {
        let mut mapping = Map::new();
        for branch in spec.branches.iter() {
            let literal = match &branch.discriminator {
                DiscriminatorRule::FieldEquals(disc) => disc.literal.clone(),
                _ => None,
            };
            if let Some(lit) = literal {
                mapping.insert(
                    lit,
                    Value::String(format!("#/$defs/{}", branch_def_key(&branch.tag))),
                );
            }
        }
        let mut d = Map::new();
        d.insert("propertyName".to_string(), Value::String(disc_field));
        if !mapping.is_empty() {
            d.insert("mapping".to_string(), Value::Object(mapping));
        }
        m.insert("discriminator".to_string(), Value::Object(d));
    }
    let _ = graph;
    m
}

fn openapi_discriminator(spec: &UnionSpec) -> Option<String> {
    let mut field: Option<String> = None;
    for branch in spec.branches.iter() {
        match &branch.discriminator {
            DiscriminatorRule::FieldEquals(disc) => {
                let _ = disc.literal.as_ref()?;
                match &field {
                    Some(prev) if prev != &disc.field_name => return None,
                    Some(_) => {}
                    None => field = Some(disc.field_name.clone()),
                }
            }
            _ => return None,
        }
    }
    field
}

fn attach_metadata(target: &mut Value, metadata: &MetadataEnvelope) {
    if metadata.is_empty() {
        return;
    }
    let Some(obj) = target.as_object_mut() else {
        return;
    };
    if let Some(doc) = &metadata.doc {
        obj.entry("description")
            .or_insert(Value::String(doc.clone()));
    }
    if !metadata.examples.is_empty() {
        obj.entry("examples").or_insert_with(|| {
            Value::Array(
                metadata
                    .examples
                    .iter()
                    .map(|e| Value::String(e.clone()))
                    .collect(),
            )
        });
    }
    if let Some(dep) = &metadata.deprecated {
        obj.entry("deprecated").or_insert(Value::Bool(true));
        obj.entry("x-golem-deprecation-note")
            .or_insert(Value::String(dep.clone()));
    }
}

/// Escape a string for use as a single JSON-Pointer token: `~` becomes `~0`
/// and `/` becomes `~1` per RFC 6901.
fn escape_pointer_token(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '~' => out.push_str("~0"),
            '/' => out.push_str("~1"),
            other => out.push(other),
        }
    }
    out
}

/// Escape a string for inclusion as a literal in a basic regex pattern.
fn regex_escape(s: &str) -> String {
    let specials: &[char] = &[
        '\\', '^', '$', '.', '|', '?', '*', '+', '(', ')', '[', ']', '{', '}',
    ];
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if specials.contains(&ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn obj<I: IntoIterator<Item = (&'static str, Value)>>(entries: I) -> Value {
    let mut map = Map::new();
    for (k, v) in entries {
        map.insert(k.to_string(), v);
    }
    Value::Object(map)
}

fn obj_inline<I: IntoIterator<Item = (&'static str, Value)>>(entries: I) -> Value {
    obj(entries)
}
