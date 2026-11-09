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

//! Renders a [`SchemaType`] in the type grammar the Kotlin SDK's value lift
//! parses (`record<name:T,...>`, `variant<Case:T|_,...>`, `list<T>`, …).
//!
//! The grammar is structural: it has no named references, so a recursive type
//! cannot be expressed and is rejected. Restrictions are not part of the
//! grammar and are dropped, because they do not change the shape of a value.

use anyhow::bail;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::{
    DiscriminatorRule, NumericBound, PathDirection, PathKind, QuantityValue, SchemaType,
};
use std::collections::HashSet;

/// Hex-encodes a string so it can be embedded as one leaf of the lowered grammar -- a path's
/// mime type, a quantity's unit, a union discriminator's literal -- without its own `,` / `<` /
/// `>` ever being mistaken for structure. The identical convention [`numeric`] already uses below
/// for a `text` refinement's `regex`, reused here rather than invented afresh: hex output is only
/// `[0-9a-f]`, so the Kotlin SDK's `splitTopLevelCommas` needs no change at all to carry it.
fn hex_leaf(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

/// `_` for `None` (the grammar's existing "no value" sentinel, as in `result<ok,_>`), else a
/// hex-encoded leaf.
fn optional_hex_leaf(value: Option<&str>) -> String {
    value.map(hex_leaf).unwrap_or_else(|| "_".to_string())
}

/// A `list<string>` of hex-encoded leaves, wrapped in its own `<...>` so top-level commas inside
/// it stay local to this field.
fn hex_leaf_list(values: &[String]) -> String {
    format!(
        "<{}>",
        values
            .iter()
            .map(|v| hex_leaf(v))
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// `_` for `None`, else [`hex_leaf_list`].
fn optional_hex_leaf_list(values: &Option<Vec<String>>) -> String {
    match values {
        Some(v) => hex_leaf_list(v),
        None => "_".to_string(),
    }
}

/// A `quantity-value` as `<mantissa,scale,unit>`, the unit hex-encoded.
fn quantity_value_literal(v: &QuantityValue) -> String {
    format!("<{},{},{}>", v.mantissa, v.scale, hex_leaf(&v.unit))
}

/// `_` for `None`, else [`quantity_value_literal`].
fn optional_quantity_value_literal(v: &Option<QuantityValue>) -> String {
    match v {
        Some(v) => quantity_value_literal(v),
        None => "_".to_string(),
    }
}

/// The SDK value-lift grammar for `typ`, resolving references in `graph`. Restrictions are dropped.
///
/// For a value the SDK only lifts, such as a method result: `Lift.kt` reads the
/// whole grammar, so every type below is renderable. Use [`sdk_wit_lowered_type`]
/// or [`sdk_wit_input_type`] for a string the SDK lowers into a schema graph.
pub(crate) fn sdk_wit_type(graph: &SchemaGraph, typ: &SchemaType) -> anyhow::Result<String> {
    render(graph, typ, &mut HashSet::new(), false, false)
}

/// Like [`sdk_wit_type`], for a value whose type the SDK lowers into a schema
/// graph rather than only lifting, such as a local config override.
pub(crate) fn sdk_wit_lowered_type(
    graph: &SchemaGraph,
    typ: &SchemaType,
) -> anyhow::Result<String> {
    render(graph, typ, &mut HashSet::new(), false, true)
}

/// Like [`sdk_wit_type`], for the type of a value a caller sends to a tool: numeric bounds, a numeric unit, text lengths and a regex are attached as
/// the refinement suffix the SDK lowers into the call's input schema, which the host compares with the command's schema.
pub(crate) fn sdk_wit_input_type(graph: &SchemaGraph, typ: &SchemaType) -> anyhow::Result<String> {
    render(graph, typ, &mut HashSet::new(), true, true)
}

fn bound_literal(bound: &NumericBound) -> String {
    match bound {
        NumericBound::Signed(v) => v.to_string(),
        NumericBound::Unsigned(v) => v.to_string(),
        NumericBound::FloatBits(bits) => f64::from_bits(*bits).to_string(),
    }
}

fn refined(base: String, fields: Vec<String>) -> String {
    if fields.is_empty() {
        base
    } else {
        format!("{base}~{}", fields.join(";"))
    }
}

fn numeric(
    base: &str,
    restrictions: &Option<golem_common::schema::schema_type::NumericRestrictions>,
    refinements: bool,
) -> anyhow::Result<String> {
    let fields = match restrictions {
        Some(r) if refinements => [("min", &r.min), ("max", &r.max)]
            .into_iter()
            .filter_map(|(name, bound)| {
                bound
                    .as_ref()
                    .map(|b| format!("{name}={}", bound_literal(b)))
            })
            // The unit goes last, as `refinedWit` writes it, and hex-encoded, so a unit holding the
            // suffix's own `;` or `=` cannot be mistaken for another field.
            .chain(r.unit.as_ref().map(|u| format!("unit={}", hex_leaf(u))))
            .collect(),
        _ => Vec::new(),
    };
    Ok(refined(base.to_string(), fields))
}

/// `path-direction`'s WIT case order (`input`/`output`/`in-out`), matching `AgentTypeModel.kt`'s
/// `pathDirectionTag`, which this must agree with exactly.
fn path_direction_name(direction: PathDirection) -> &'static str {
    match direction {
        PathDirection::Input => "input",
        PathDirection::Output => "output",
        PathDirection::InOut => "in-out",
    }
}

/// `path-kind`'s WIT case order (`file`/`directory`/`any`), matching `AgentTypeModel.kt`'s
/// `pathKindTag`, which this must agree with exactly.
fn path_kind_name(kind: PathKind) -> &'static str {
    match kind {
        PathKind::File => "file",
        PathKind::Directory => "directory",
        PathKind::Any => "any",
    }
}

/// `prefix<hex>` / `suffix<hex>` / `contains<hex>` / `regex<hex>` /
/// `field-equals<hex-name,hex-literal-or-_>` / `field-absent<hex-name>`, matching
/// `AgentTypeModel.kt`'s `lowerDiscriminatorRuleInto`, which this must agree with exactly.
fn discriminator_rule_literal(rule: &DiscriminatorRule) -> String {
    match rule {
        DiscriminatorRule::Prefix { prefix } => format!("prefix<{}>", hex_leaf(prefix)),
        DiscriminatorRule::Suffix { suffix } => format!("suffix<{}>", hex_leaf(suffix)),
        DiscriminatorRule::Contains { substring } => format!("contains<{}>", hex_leaf(substring)),
        DiscriminatorRule::Regex { regex } => format!("regex<{}>", hex_leaf(regex)),
        DiscriminatorRule::FieldEquals(fd) => format!(
            "field-equals<{},{}>",
            hex_leaf(&fd.field_name),
            optional_hex_leaf(fd.literal.as_deref()),
        ),
        DiscriminatorRule::FieldAbsent { field_name } => {
            format!("field-absent<{}>", hex_leaf(field_name))
        }
    }
}

/// The grammar splits on `,` and `:` and nests on `<` `>`, and has no escape, so a name holding one cannot be expressed.
fn checked_name(name: &str) -> anyhow::Result<&str> {
    if name.contains([',', ':', '<', '>']) {
        bail!(
            "The name `{name}` holds a character (`,` `:` `<` `>`) that the Kotlin SDK's value grammar cannot express, so a guest bridge cannot use it"
        );
    }
    Ok(name)
}

/// The longest grammar string this module will build. The grammar is structural,
/// so a definition shared by several fields is expanded afresh at each use: a
/// chain of definitions that each reference the previous one twice doubles in
/// size per link. Without a bound the generator would appear to hang while
/// building a vast literal, so it fails with a clear message instead.
const RENDERED_LENGTH_MAX: usize = 64 * 1024;

fn render(
    graph: &SchemaGraph,
    typ: &SchemaType,
    visiting: &mut HashSet<String>,
    refinements: bool,
    lowered: bool,
) -> anyhow::Result<String> {
    if let SchemaType::Ref { id, .. } = typ {
        if !visiting.insert(id.0.clone()) {
            bail!(
                "The recursive type `{}` cannot be used in a Kotlin guest bridge: the SDK's value grammar has no named references",
                id.0
            );
        }
        let resolved = graph.resolve_ref(typ)?;
        let rendered = render(graph, resolved, visiting, refinements, lowered);
        visiting.remove(&id.0);
        let rendered = rendered?;
        if rendered.len() > RENDERED_LENGTH_MAX {
            bail!(
                "The type `{}` expands to more than {RENDERED_LENGTH_MAX} characters in the SDK's value grammar, which has no named references: every shared definition is written out at each use",
                id.0
            );
        }
        return Ok(rendered);
    }
    let mut child = |t: &SchemaType| render(graph, t, visiting, refinements, lowered);
    Ok(match typ {
        SchemaType::Bool { .. } => "bool".to_string(),
        SchemaType::S8 { restrictions, .. } => numeric("s8", restrictions, refinements)?,
        SchemaType::S16 { restrictions, .. } => numeric("s16", restrictions, refinements)?,
        SchemaType::S32 { restrictions, .. } => numeric("s32", restrictions, refinements)?,
        SchemaType::S64 { restrictions, .. } => numeric("s64", restrictions, refinements)?,
        SchemaType::U8 { restrictions, .. } => numeric("u8", restrictions, refinements)?,
        SchemaType::U16 { restrictions, .. } => numeric("u16", restrictions, refinements)?,
        SchemaType::U32 { restrictions, .. } => numeric("u32", restrictions, refinements)?,
        SchemaType::U64 { restrictions, .. } => numeric("u64", restrictions, refinements)?,
        SchemaType::F32 { restrictions, .. } => numeric("f32", restrictions, refinements)?,
        SchemaType::F64 { restrictions, .. } => numeric("f64", restrictions, refinements)?,
        SchemaType::Char { .. } => "char".to_string(),
        SchemaType::String { .. } => "string".to_string(),
        SchemaType::Record { fields, .. } => {
            let parts = fields
                .iter()
                .map(|f| Ok(format!("{}:{}", checked_name(&f.name)?, child(&f.body)?)))
                .collect::<anyhow::Result<Vec<_>>>()?;
            format!("record<{}>", parts.join(","))
        }
        SchemaType::Variant { cases, .. } => {
            let parts = cases
                .iter()
                .map(|c| {
                    let payload = match &c.payload {
                        Some(p) => child(p)?,
                        None => "_".to_string(),
                    };
                    Ok(format!("{}:{payload}", checked_name(&c.name)?))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            format!("variant<{}>", parts.join(","))
        }
        SchemaType::Enum { cases, .. } => {
            for case in cases {
                checked_name(case)?;
            }
            if cases.is_empty() {
                "enum".to_string()
            } else {
                format!("enum<{}>", cases.join(","))
            }
        }
        SchemaType::Flags { flags, .. } => {
            for flag in flags {
                checked_name(flag)?;
            }
            format!("flags<{}>", flags.join(","))
        }
        SchemaType::Tuple { elements, .. } => {
            let parts = elements
                .iter()
                .map(&mut child)
                .collect::<anyhow::Result<Vec<_>>>()?;
            format!("tuple<{}>", parts.join(","))
        }
        SchemaType::List { element, .. } => format!("list<{}>", child(element)?),
        SchemaType::FixedList {
            element, length, ..
        } => format!("fixed-list<{},{length}>", child(element)?),
        SchemaType::Map { key, value, .. } => format!("map<{},{}>", child(key)?, child(value)?),
        SchemaType::Option { inner, .. } => format!("option<{}>", child(inner)?),
        SchemaType::Result { spec, .. } => {
            let ok = match spec.ok.as_deref() {
                Some(t) => child(t)?,
                None => "_".to_string(),
            };
            let err = match spec.err.as_deref() {
                Some(t) => child(t)?,
                None => "_".to_string(),
            };
            format!("result<{ok},{err}>")
        }
        SchemaType::Text { restrictions, .. } => {
            if !refinements {
                "text".to_string()
            } else {
                let base = match restrictions.languages.as_deref() {
                    Some(languages) if !languages.is_empty() => {
                        format!("text<{}>", languages.join(","))
                    }
                    _ => "text".to_string(),
                };
                let mut fields = Vec::new();
                if let Some(v) = restrictions.min_length {
                    fields.push(format!("minLength={v}"));
                }
                if let Some(v) = restrictions.max_length {
                    fields.push(format!("maxLength={v}"));
                }
                if let Some(regex) = &restrictions.regex {
                    let hex: String = regex.bytes().map(|b| format!("{b:02x}")).collect();
                    fields.push(format!("regex={hex}"));
                }
                refined(base, fields)
            }
        }
        SchemaType::Binary { restrictions, .. } => {
            if !refinements {
                "binary".to_string()
            } else if restrictions.min_bytes.is_some() || restrictions.max_bytes.is_some() {
                bail!(
                    "The byte limits of a binary cannot be sent in a tool call by a Kotlin bridge: the SDK's value grammar cannot carry them, and the host compares the call's input schema with the command's exactly"
                );
            } else {
                match restrictions.mime_types.as_deref() {
                    Some(mimes) if !mimes.is_empty() => format!("binary<{}>", mimes.join(",")),
                    _ => "binary".to_string(),
                }
            }
        }
        SchemaType::Path { spec, .. } => {
            if !lowered {
                // The lift direction only ever needs the bare tag: a decoded value is a plain
                // string, and `Lift.kt` never reads the spec for it.
                "path".to_string()
            } else {
                format!(
                    "path<{},{},{},{}>",
                    path_direction_name(spec.direction),
                    path_kind_name(spec.kind),
                    optional_hex_leaf_list(&spec.allowed_mime_types),
                    optional_hex_leaf_list(&spec.allowed_extensions),
                )
            }
        }
        SchemaType::Url { restrictions, .. } => {
            if !lowered
                || (restrictions.allowed_schemes.is_none() && restrictions.allowed_hosts.is_none())
            {
                // The lift direction needs only the bare tag, as the `path` branch above does: a
                // decoded url value is a plain string. An unrestricted url takes the bare tag too,
                // because the SDK reads `url` as the same empty `url-restrictions` that `url<_,_>`
                // builds, and the host compares the two schemas exactly.
                "url".to_string()
            } else {
                format!(
                    "url<{},{}>",
                    optional_hex_leaf_list(&restrictions.allowed_schemes),
                    optional_hex_leaf_list(&restrictions.allowed_hosts),
                )
            }
        }
        SchemaType::Uuid { .. } => "uuid".to_string(),
        SchemaType::Datetime { .. } => "datetime".to_string(),
        SchemaType::Duration { .. } => "duration".to_string(),
        SchemaType::Quantity { spec, .. } => {
            if !lowered {
                // Lift only ever needs the bare tag: a decoded value is {mantissa, scale, unit},
                // and `Lift.kt` never reads the spec for it.
                "quantity".to_string()
            } else {
                format!(
                    "quantity<{},{},{},{}>",
                    hex_leaf(&spec.base_unit),
                    hex_leaf_list(&spec.allowed_suffixes),
                    optional_quantity_value_literal(&spec.min),
                    optional_quantity_value_literal(&spec.max),
                )
            }
        }
        SchemaType::Union { spec, .. } => {
            if !lowered {
                let parts = spec
                    .branches
                    .iter()
                    .map(|b| Ok(format!("{}:{}", checked_name(&b.tag)?, child(&b.body)?)))
                    .collect::<anyhow::Result<Vec<_>>>()?;
                format!("union<{}>", parts.join(","))
            } else {
                let parts = spec
                    .branches
                    .iter()
                    .map(|b| {
                        Ok(format!(
                            "<{},{},{}>",
                            hex_leaf(&b.tag),
                            child(&b.body)?,
                            discriminator_rule_literal(&b.discriminator),
                        ))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                format!("union<{}>", parts.join(","))
            }
        }
        SchemaType::Secret { .. }
        | SchemaType::QuotaToken { .. }
        | SchemaType::PermissionCard { .. }
        | SchemaType::Future { .. }
        | SchemaType::Stream { .. } => {
            bail!("A host-managed value cannot be used in a Kotlin guest bridge: {typ:?}")
        }
        SchemaType::Ref { .. } => unreachable!("references are resolved above"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::{
        MetadataEnvelope, NamedFieldType, ResultSpec, SchemaTypeDef, TypeId, VariantCaseType,
    };
    use test_r::test;

    fn graph(defs: Vec<(&str, SchemaType)>) -> SchemaGraph {
        SchemaGraph {
            defs: defs
                .into_iter()
                .map(|(id, body)| SchemaTypeDef {
                    id: TypeId::new(id),
                    name: None,
                    body,
                })
                .collect(),
            root: SchemaType::record(vec![]),
        }
    }

    fn field(name: &str, body: SchemaType) -> NamedFieldType {
        NamedFieldType {
            name: name.to_string(),
            body,
            metadata: MetadataEnvelope::default(),
        }
    }

    #[test]
    fn scalars_and_collections_use_the_sdk_grammar() {
        let g = graph(vec![]);
        let t = SchemaType::map(
            SchemaType::string(),
            SchemaType::list(SchemaType::option(SchemaType::u32())),
        );
        assert_eq!(
            sdk_wit_type(&g, &t).unwrap(),
            "map<string,list<option<u32>>>"
        );
        let t = SchemaType::tuple(vec![SchemaType::s8(), SchemaType::char()]);
        assert_eq!(sdk_wit_type(&g, &t).unwrap(), "tuple<s8,char>");
        assert_eq!(
            sdk_wit_type(&g, &SchemaType::fixed_list(SchemaType::f64(), 4)).unwrap(),
            "fixed-list<f64,4>"
        );
    }

    #[test]
    fn named_composites_are_expanded_inline() {
        let g = graph(vec![
            (
                "person",
                SchemaType::record(vec![
                    field("name", SchemaType::string()),
                    field("color", SchemaType::ref_to(TypeId::new("color"))),
                ]),
            ),
            (
                "color",
                SchemaType::r#enum(vec!["red".into(), "green".into()]),
            ),
        ]);
        let t = SchemaType::ref_to(TypeId::new("person"));
        assert_eq!(
            sdk_wit_type(&g, &t).unwrap(),
            "record<name:string,color:enum<red,green>>"
        );
    }

    #[test]
    fn variant_payloads_results_and_unions_use_underscore_for_nothing() {
        let g = graph(vec![]);
        let v = SchemaType::variant(vec![
            VariantCaseType {
                name: "home".into(),
                payload: Some(SchemaType::string()),
                metadata: MetadataEnvelope::default(),
            },
            VariantCaseType {
                name: "unknown".into(),
                payload: None,
                metadata: MetadataEnvelope::default(),
            },
        ]);
        assert_eq!(
            sdk_wit_type(&g, &v).unwrap(),
            "variant<home:string,unknown:_>"
        );
        let r = SchemaType::result(ResultSpec {
            ok: None,
            err: Some(Box::new(SchemaType::string())),
        });
        assert_eq!(sdk_wit_type(&g, &r).unwrap(), "result<_,string>");
        assert_eq!(
            sdk_wit_type(&g, &SchemaType::flags(vec!["a".into()])).unwrap(),
            "flags<a>"
        );
    }

    #[test]
    fn a_shared_definition_used_twice_is_not_mistaken_for_recursion() {
        let g = graph(vec![("c", SchemaType::r#enum(vec!["a".into()]))]);
        let t = SchemaType::tuple(vec![
            SchemaType::ref_to(TypeId::new("c")),
            SchemaType::ref_to(TypeId::new("c")),
        ]);
        assert_eq!(sdk_wit_type(&g, &t).unwrap(), "tuple<enum<a>,enum<a>>");
    }

    #[test]
    fn a_recursive_type_is_rejected() {
        let g = graph(vec![(
            "tree",
            SchemaType::record(vec![field(
                "children",
                SchemaType::list(SchemaType::ref_to(TypeId::new("tree"))),
            )]),
        )]);
        let error = sdk_wit_type(&g, &SchemaType::ref_to(TypeId::new("tree"))).unwrap_err();
        assert!(
            error.to_string().contains("recursive type `tree`"),
            "{error}"
        );
    }

    #[test]
    fn the_input_grammar_carries_bounds_lengths_and_a_regex_and_the_plain_grammar_drops_them() {
        use golem_common::schema::schema_type::{NumericRestrictions, TextRestrictions};
        let g = graph(vec![]);
        let bounded = SchemaType::S32 {
            restrictions: Some(NumericRestrictions {
                min: Some(NumericBound::Signed(-5)),
                max: Some(NumericBound::Signed(10)),
                unit: None,
            }),
            metadata: MetadataEnvelope::default(),
        };
        let floaty = SchemaType::F64 {
            restrictions: Some(NumericRestrictions {
                min: Some(NumericBound::FloatBits(0.5f64.to_bits())),
                max: None,
                unit: None,
            }),
            metadata: MetadataEnvelope::default(),
        };
        let text = SchemaType::text(TextRestrictions {
            languages: Some(vec!["en".into(), "de".into()]),
            min_length: Some(2),
            max_length: None,
            regex: Some("^a$".into()),
        });
        let record = SchemaType::record(vec![
            field("a", bounded),
            field("b", floaty),
            field("c", text),
        ]);
        assert_eq!(
            sdk_wit_input_type(&g, &record).unwrap(),
            "record<a:s32~min=-5;max=10,b:f64~min=0.5,c:text<en,de>~minLength=2;regex=5e6124>"
        );
        assert_eq!(
            sdk_wit_type(&g, &record).unwrap(),
            "record<a:s32,b:f64,c:text>"
        );
    }

    #[test]
    fn the_input_grammar_rejects_restrictions_it_cannot_carry_and_renders_binary_mime_types() {
        use golem_common::schema::schema_type::BinaryRestrictions;
        let g = graph(vec![]);
        let sized = SchemaType::binary(BinaryRestrictions {
            mime_types: None,
            min_bytes: None,
            max_bytes: Some(10),
        });
        let error = sdk_wit_input_type(&g, &sized).unwrap_err();
        assert!(error.to_string().contains("cannot carry"), "{error}");
        assert!(
            sdk_wit_type(&g, &sized).is_ok(),
            "a byte limit is fine in a value position"
        );
        let mimes = SchemaType::binary(BinaryRestrictions {
            mime_types: Some(vec!["image/png".into(), "image/jpeg".into()]),
            min_bytes: None,
            max_bytes: None,
        });
        assert_eq!(
            sdk_wit_input_type(&g, &mimes).unwrap(),
            "binary<image/png,image/jpeg>"
        );
        assert_eq!(sdk_wit_type(&g, &mimes).unwrap(), "binary");
    }

    /// A url's schemes and hosts reach the SDK hex-encoded on the lowered path, the same way a
    /// path's spec does, so a call's input schema carries the `url-restrictions` the command
    /// declares. The value position keeps the bare tag, which is all the lift reads.
    #[test]
    fn the_lowered_grammar_carries_url_schemes_and_hosts_hex_encoded() {
        use golem_common::schema::schema_type::UrlRestrictions;
        let g = graph(vec![]);
        let restricted = SchemaType::url(UrlRestrictions {
            allowed_schemes: Some(vec!["https".into()]),
            allowed_hosts: Some(vec!["example.test".into()]),
        });
        // "https" and "example.test" as hex-encoded UTF-8 leaves.
        assert_eq!(
            sdk_wit_input_type(&g, &restricted).unwrap(),
            "url<<6874747073>,<6578616d706c652e74657374>>"
        );
        assert_eq!(
            sdk_wit_lowered_type(&g, &restricted).unwrap(),
            sdk_wit_input_type(&g, &restricted).unwrap()
        );
        assert_eq!(sdk_wit_type(&g, &restricted).unwrap(), "url");

        let schemes_only = SchemaType::url(UrlRestrictions {
            allowed_schemes: Some(vec!["https".into()]),
            allowed_hosts: None,
        });
        assert_eq!(
            sdk_wit_input_type(&g, &schemes_only).unwrap(),
            "url<<6874747073>,_>"
        );

        let unrestricted = SchemaType::url(UrlRestrictions {
            allowed_schemes: None,
            allowed_hosts: None,
        });
        assert_eq!(sdk_wit_input_type(&g, &unrestricted).unwrap(), "url");
    }

    /// A numeric unit reaches the SDK as a hex-encoded refinement field, so a tool call's input
    /// schema carries the same `numeric-restrictions.unit` the command declares.
    #[test]
    fn the_input_grammar_carries_a_numeric_unit_hex_encoded() {
        use golem_common::schema::schema_type::NumericRestrictions;
        let g = graph(vec![]);
        let plain = SchemaType::S32 {
            restrictions: Some(NumericRestrictions {
                min: None,
                max: None,
                unit: Some("ms".into()),
            }),
            metadata: MetadataEnvelope::default(),
        };
        assert_eq!(sdk_wit_input_type(&g, &plain).unwrap(), "s32~unit=6d73");
        // The unit goes after the bounds, as `refinedWit` writes it.
        let bounded = SchemaType::U32 {
            restrictions: Some(NumericRestrictions {
                min: Some(NumericBound::Unsigned(0)),
                max: Some(NumericBound::Unsigned(60000)),
                unit: Some("ms".into()),
            }),
            metadata: MetadataEnvelope::default(),
        };
        assert_eq!(
            sdk_wit_input_type(&g, &bounded).unwrap(),
            "u32~min=0;max=60000;unit=6d73"
        );
        // A unit holding the suffix's own separators survives, because it is hex-encoded.
        let hostile = SchemaType::F32 {
            restrictions: Some(NumericRestrictions {
                min: None,
                max: None,
                unit: Some("m/s;x=1".into()),
            }),
            metadata: MetadataEnvelope::default(),
        };
        assert_eq!(
            sdk_wit_input_type(&g, &hostile).unwrap(),
            "f32~unit=6d2f733b783d31"
        );
        // The unit is metadata, so it stays out of a value position.
        assert_eq!(sdk_wit_type(&g, &plain).unwrap(), "s32");
    }

    #[test]
    fn a_name_the_grammar_cannot_express_is_rejected() {
        let g = graph(vec![]);
        for bad in ["a:b", "x,y", "<none>"] {
            let t = SchemaType::record(vec![field(bad, SchemaType::string())]);
            let error = sdk_wit_type(&g, &t).unwrap_err();
            assert!(
                error.to_string().contains("cannot express"),
                "{bad}: {error}"
            );
        }
        let e = SchemaType::r#enum(vec!["ok".into(), "a,b".into()]);
        assert!(sdk_wit_type(&g, &e).is_err());
    }

    #[test]
    fn host_managed_values_are_rejected() {
        let g = graph(vec![]);
        let error = sdk_wit_type(&g, &SchemaType::stream(Some(SchemaType::string()))).unwrap_err();
        assert!(error.to_string().contains("host-managed"), "{error}");
    }
}
