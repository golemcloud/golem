// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

//! Schema evolution validation for agent types.
//!
//! When a component is redeployed with a new version, this module compares the
//! old and new agent type schemas and produces warnings about positionally
//! incompatible changes that could corrupt existing agent IDs.

use crate::base_model::agent::AgentTypeName;
use crate::schema::agent::{AgentTypeSchema, InputSchema};
use crate::schema::graph::SchemaGraph;
use crate::schema::schema_type::SchemaType;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct SchemaEvolutionWarning {
    pub agent_type_name: AgentTypeName,
    pub path: String,
    pub description: String,
}

impl fmt::Display for SchemaEvolutionWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Agent '{}' at {}: {}",
            self.agent_type_name, self.path, self.description
        )
    }
}

/// Compare old and new agent type lists and produce warnings about
/// positionally incompatible schema changes.
pub fn validate_schema_evolution(
    old_agent_types: &[AgentTypeSchema],
    new_agent_types: &[AgentTypeSchema],
) -> Vec<SchemaEvolutionWarning> {
    let mut warnings = Vec::new();

    for old in old_agent_types {
        match new_agent_types
            .iter()
            .find(|n| n.type_name == old.type_name)
        {
            None => {
                warnings.push(SchemaEvolutionWarning {
                    agent_type_name: old.type_name.clone(),
                    path: "constructor".to_string(),
                    description: "Agent type removed".to_string(),
                });
            }
            Some(new) => {
                validate_input_schema(
                    &old.type_name,
                    "constructor",
                    &old.schema,
                    &old.constructor.input_schema,
                    &new.schema,
                    &new.constructor.input_schema,
                    &mut warnings,
                );
            }
        }
    }

    warnings
}

#[allow(clippy::too_many_arguments)]
fn validate_input_schema(
    name: &AgentTypeName,
    path: &str,
    old_graph: &SchemaGraph,
    old: &InputSchema,
    new_graph: &SchemaGraph,
    new: &InputSchema,
    warnings: &mut Vec<SchemaEvolutionWarning>,
) {
    let old_fields = old.fields();
    let new_fields = new.fields();

    if new_fields.len() < old_fields.len() {
        warnings.push(SchemaEvolutionWarning {
            agent_type_name: name.clone(),
            path: path.to_string(),
            description: format!(
                "Elements removed (was {}, now {})",
                old_fields.len(),
                new_fields.len()
            ),
        });
        return;
    }

    for (i, (old_field, new_field)) in old_fields.iter().zip(new_fields.iter()).enumerate() {
        let elem_path = format!("{}.elements[{}]", path, i);
        validate_schema_type(
            name,
            &elem_path,
            old_graph,
            &old_field.schema,
            new_graph,
            &new_field.schema,
            warnings,
        );
    }
}

fn discriminant_name(typ: &SchemaType) -> &'static str {
    match typ {
        SchemaType::Ref { .. } => "Ref",
        SchemaType::Bool { .. } => "Bool",
        SchemaType::S8 { .. } => "S8",
        SchemaType::S16 { .. } => "S16",
        SchemaType::S32 { .. } => "S32",
        SchemaType::S64 { .. } => "S64",
        SchemaType::U8 { .. } => "U8",
        SchemaType::U16 { .. } => "U16",
        SchemaType::U32 { .. } => "U32",
        SchemaType::U64 { .. } => "U64",
        SchemaType::F32 { .. } => "F32",
        SchemaType::F64 { .. } => "F64",
        SchemaType::Char { .. } => "Char",
        SchemaType::String { .. } => "String",
        SchemaType::Record { .. } => "Record",
        SchemaType::Variant { .. } => "Variant",
        SchemaType::Enum { .. } => "Enum",
        SchemaType::Flags { .. } => "Flags",
        SchemaType::Tuple { .. } => "Tuple",
        SchemaType::List { .. } => "List",
        SchemaType::FixedList { .. } => "FixedList",
        SchemaType::Map { .. } => "Map",
        SchemaType::Option { .. } => "Option",
        SchemaType::Result { .. } => "Result",
        SchemaType::Text { .. } => "Text",
        SchemaType::Binary { .. } => "Binary",
        SchemaType::Path { .. } => "Path",
        SchemaType::Url { .. } => "Url",
        SchemaType::Datetime { .. } => "Datetime",
        SchemaType::Duration { .. } => "Duration",
        SchemaType::Quantity { .. } => "Quantity",
        SchemaType::Union { .. } => "Union",
        SchemaType::Secret { .. } => "Secret",
        SchemaType::QuotaToken { .. } => "QuotaToken",
        SchemaType::PermissionCard { .. } => "PermissionCard",
        SchemaType::Future { .. } => "Future",
        SchemaType::Stream { .. } => "Stream",
    }
}

/// Resolve any [`SchemaType::Ref`] chain against the owning graph, returning
/// the original (possibly ref) type if resolution fails so that downstream
/// comparison still produces a stable result.
fn resolve<'a>(graph: &'a SchemaGraph, ty: &'a SchemaType) -> &'a SchemaType {
    graph.resolve_ref(ty).unwrap_or(ty)
}

fn is_option(graph: &SchemaGraph, ty: &SchemaType) -> bool {
    matches!(resolve(graph, ty), SchemaType::Option { .. })
}

#[allow(clippy::too_many_arguments)]
fn validate_schema_type(
    name: &AgentTypeName,
    path: &str,
    old_graph: &SchemaGraph,
    old: &SchemaType,
    new_graph: &SchemaGraph,
    new: &SchemaType,
    warnings: &mut Vec<SchemaEvolutionWarning>,
) {
    let old = resolve(old_graph, old);
    let new = resolve(new_graph, new);

    let old_disc = discriminant_name(old);
    let new_disc = discriminant_name(new);
    if old_disc != new_disc {
        warnings.push(SchemaEvolutionWarning {
            agent_type_name: name.clone(),
            path: path.to_string(),
            description: format!("Type changed from {} to {}", old_disc, new_disc),
        });
        return;
    }

    match (old, new) {
        (
            SchemaType::Record {
                fields: old_fields, ..
            },
            SchemaType::Record {
                fields: new_fields, ..
            },
        ) => {
            if new_fields.len() < old_fields.len() {
                warnings.push(SchemaEvolutionWarning {
                    agent_type_name: name.clone(),
                    path: path.to_string(),
                    description: format!(
                        "Record fields removed (was {}, now {})",
                        old_fields.len(),
                        new_fields.len()
                    ),
                });
                return;
            }
            for (i, (old_f, new_f)) in old_fields.iter().zip(new_fields.iter()).enumerate() {
                let field_path = format!("{}.fields[{}]", path, i);
                validate_schema_type(
                    name,
                    &field_path,
                    old_graph,
                    &old_f.body,
                    new_graph,
                    &new_f.body,
                    warnings,
                );
            }
            for (i, new_f) in new_fields.iter().enumerate().skip(old_fields.len()) {
                if !is_option(new_graph, &new_f.body) {
                    let field_path = format!("{}.fields[{}]", path, i);
                    warnings.push(SchemaEvolutionWarning {
                        agent_type_name: name.clone(),
                        path: field_path,
                        description: "Appended non-Option record field".to_string(),
                    });
                }
            }
        }
        (
            SchemaType::Tuple {
                elements: old_items,
                ..
            },
            SchemaType::Tuple {
                elements: new_items,
                ..
            },
        ) => {
            if new_items.len() < old_items.len() {
                warnings.push(SchemaEvolutionWarning {
                    agent_type_name: name.clone(),
                    path: path.to_string(),
                    description: format!(
                        "Tuple items removed (was {}, now {})",
                        old_items.len(),
                        new_items.len()
                    ),
                });
                return;
            }
            for (i, (old_item, new_item)) in old_items.iter().zip(new_items.iter()).enumerate() {
                let item_path = format!("{}.items[{}]", path, i);
                validate_schema_type(
                    name, &item_path, old_graph, old_item, new_graph, new_item, warnings,
                );
            }
            for (i, new_item) in new_items.iter().enumerate().skip(old_items.len()) {
                if !is_option(new_graph, new_item) {
                    let item_path = format!("{}.items[{}]", path, i);
                    warnings.push(SchemaEvolutionWarning {
                        agent_type_name: name.clone(),
                        path: item_path,
                        description: "Appended non-Option tuple item".to_string(),
                    });
                }
            }
        }
        (
            SchemaType::Variant {
                cases: old_cases, ..
            },
            SchemaType::Variant {
                cases: new_cases, ..
            },
        ) => {
            if new_cases.len() < old_cases.len() {
                warnings.push(SchemaEvolutionWarning {
                    agent_type_name: name.clone(),
                    path: path.to_string(),
                    description: format!(
                        "Variant cases removed (was {}, now {})",
                        old_cases.len(),
                        new_cases.len()
                    ),
                });
                return;
            }
            for (i, (old_case, new_case)) in old_cases.iter().zip(new_cases.iter()).enumerate() {
                let case_path = format!("{}.cases[{}]", path, i);
                match (&old_case.payload, &new_case.payload) {
                    (None, Some(_)) => {
                        warnings.push(SchemaEvolutionWarning {
                            agent_type_name: name.clone(),
                            path: case_path,
                            description: "Variant case gained a payload".to_string(),
                        });
                    }
                    (Some(_), None) => {
                        warnings.push(SchemaEvolutionWarning {
                            agent_type_name: name.clone(),
                            path: case_path,
                            description: "Variant case lost its payload".to_string(),
                        });
                    }
                    (Some(old_t), Some(new_t)) => {
                        validate_schema_type(
                            name, &case_path, old_graph, old_t, new_graph, new_t, warnings,
                        );
                    }
                    (None, None) => {}
                }
            }
        }
        (
            SchemaType::Enum {
                cases: old_cases, ..
            },
            SchemaType::Enum {
                cases: new_cases, ..
            },
        ) if new_cases.len() < old_cases.len() => {
            warnings.push(SchemaEvolutionWarning {
                agent_type_name: name.clone(),
                path: path.to_string(),
                description: format!(
                    "Enum cases removed (was {}, now {})",
                    old_cases.len(),
                    new_cases.len()
                ),
            });
        }
        (
            SchemaType::Flags {
                flags: old_flags, ..
            },
            SchemaType::Flags {
                flags: new_flags, ..
            },
        ) if new_flags.len() < old_flags.len() => {
            warnings.push(SchemaEvolutionWarning {
                agent_type_name: name.clone(),
                path: path.to_string(),
                description: format!(
                    "Flags removed (was {}, now {})",
                    old_flags.len(),
                    new_flags.len()
                ),
            });
        }
        (
            SchemaType::List {
                element: old_inner, ..
            },
            SchemaType::List {
                element: new_inner, ..
            },
        ) => {
            validate_schema_type(
                name, path, old_graph, old_inner, new_graph, new_inner, warnings,
            );
        }
        (
            SchemaType::FixedList {
                element: old_inner, ..
            },
            SchemaType::FixedList {
                element: new_inner, ..
            },
        ) => {
            validate_schema_type(
                name, path, old_graph, old_inner, new_graph, new_inner, warnings,
            );
        }
        (
            SchemaType::Map {
                key: old_key,
                value: old_value,
                ..
            },
            SchemaType::Map {
                key: new_key,
                value: new_value,
                ..
            },
        ) => {
            validate_schema_type(
                name,
                &format!("{}.key", path),
                old_graph,
                old_key,
                new_graph,
                new_key,
                warnings,
            );
            validate_schema_type(
                name,
                &format!("{}.value", path),
                old_graph,
                old_value,
                new_graph,
                new_value,
                warnings,
            );
        }
        (
            SchemaType::Option {
                inner: old_inner, ..
            },
            SchemaType::Option {
                inner: new_inner, ..
            },
        ) => {
            validate_schema_type(
                name, path, old_graph, old_inner, new_graph, new_inner, warnings,
            );
        }
        (SchemaType::Result { spec: old_spec, .. }, SchemaType::Result { spec: new_spec, .. }) => {
            match (&old_spec.ok, &new_spec.ok) {
                (None, Some(_)) | (Some(_), None) => {
                    warnings.push(SchemaEvolutionWarning {
                        agent_type_name: name.clone(),
                        path: format!("{}.ok", path),
                        description: "Result ok payload presence changed".to_string(),
                    });
                }
                (Some(old_ok), Some(new_ok)) => {
                    validate_schema_type(
                        name,
                        &format!("{}.ok", path),
                        old_graph,
                        old_ok,
                        new_graph,
                        new_ok,
                        warnings,
                    );
                }
                (None, None) => {}
            }
            match (&old_spec.err, &new_spec.err) {
                (None, Some(_)) | (Some(_), None) => {
                    warnings.push(SchemaEvolutionWarning {
                        agent_type_name: name.clone(),
                        path: format!("{}.err", path),
                        description: "Result err payload presence changed".to_string(),
                    });
                }
                (Some(old_err), Some(new_err)) => {
                    validate_schema_type(
                        name,
                        &format!("{}.err", path),
                        old_graph,
                        old_err,
                        new_graph,
                        new_err,
                        warnings,
                    );
                }
                (None, None) => {}
            }
        }
        // Same-discriminant primitives, rich scalars, capabilities, unions and
        // P3 stubs are treated as compatible (no positional drift).
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_model::agent::{AgentMode, AgentTypeName, Snapshotting};
    use crate::model::Empty;
    use crate::schema::agent::{AgentConstructorSchema, NamedField};
    use crate::schema::graph::SchemaGraph;
    use crate::schema::schema_type::{ResultSpec, SchemaType, VariantCaseType};
    use test_r::test;

    fn field(name: &str, schema: SchemaType) -> NamedField {
        NamedField {
            name: name.to_string(),
            source: Default::default(),
            schema,
            metadata: Default::default(),
        }
    }

    fn rec(fields: Vec<(&str, SchemaType)>) -> SchemaType {
        SchemaType::record(
            fields
                .into_iter()
                .map(|(n, t)| crate::schema::schema_type::NamedFieldType {
                    name: n.to_string(),
                    body: t,
                    metadata: Default::default(),
                })
                .collect(),
        )
    }

    fn unit_case(name: &str) -> VariantCaseType {
        VariantCaseType {
            name: name.to_string(),
            payload: None,
            metadata: Default::default(),
        }
    }

    fn case(name: &str, payload: SchemaType) -> VariantCaseType {
        VariantCaseType {
            name: name.to_string(),
            payload: Some(payload),
            metadata: Default::default(),
        }
    }

    fn result_ok(ok: SchemaType) -> SchemaType {
        SchemaType::result(ResultSpec {
            ok: Some(Box::new(ok)),
            err: None,
        })
    }

    fn result(ok: SchemaType, err: SchemaType) -> SchemaType {
        SchemaType::result(ResultSpec {
            ok: Some(Box::new(ok)),
            err: Some(Box::new(err)),
        })
    }

    fn make_agent_type(name: &str, fields: Vec<(&str, SchemaType)>) -> AgentTypeSchema {
        AgentTypeSchema {
            type_name: AgentTypeName(name.to_string()),
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(
                    fields.into_iter().map(|(n, t)| field(n, t)).collect(),
                ),
            },
            methods: Vec::new(),
            dependencies: Vec::new(),
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: Vec::new(),
        }
    }

    #[test]
    fn no_changes_no_warnings() {
        let old = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
        let new = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
        assert!(validate_schema_evolution(&old, &new).is_empty());
    }

    #[test]
    fn agent_type_removed() {
        let old = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
        let new = vec![];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("removed"));
    }

    #[test]
    fn agent_type_added_no_warning() {
        let old = vec![];
        let new = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
        assert!(validate_schema_evolution(&old, &new).is_empty());
    }

    #[test]
    fn element_removed() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::u32()), ("y", SchemaType::string())],
        )];
        let new = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("removed"));
    }

    #[test]
    fn element_type_changed() {
        let old = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
        let new = vec![make_agent_type("A", vec![("x", SchemaType::string())])];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Type changed"));
    }

    #[test]
    fn record_field_removed() {
        let old = vec![make_agent_type(
            "A",
            vec![(
                "x",
                rec(vec![("a", SchemaType::u32()), ("b", SchemaType::string())]),
            )],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![("x", rec(vec![("a", SchemaType::u32())]))],
        )];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("fields removed"));
    }

    #[test]
    fn record_field_type_changed() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", rec(vec![("a", SchemaType::u32())]))],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![("x", rec(vec![("a", SchemaType::string())]))],
        )];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Type changed"));
    }

    #[test]
    fn record_append_option_field_compatible() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", rec(vec![("a", SchemaType::u32())]))],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![(
                "x",
                rec(vec![
                    ("a", SchemaType::u32()),
                    ("b", SchemaType::option(SchemaType::string())),
                ]),
            )],
        )];
        assert!(validate_schema_evolution(&old, &new).is_empty());
    }

    #[test]
    fn record_append_non_option_field_warns() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", rec(vec![("a", SchemaType::u32())]))],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![(
                "x",
                rec(vec![("a", SchemaType::u32()), ("b", SchemaType::string())]),
            )],
        )];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("non-Option"));
    }

    #[test]
    fn variant_case_removed() {
        let old = vec![make_agent_type(
            "A",
            vec![(
                "x",
                SchemaType::variant(vec![unit_case("A"), unit_case("B")]),
            )],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::variant(vec![unit_case("A")]))],
        )];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("cases removed"));
    }

    #[test]
    fn variant_payload_added() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::variant(vec![unit_case("A")]))],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::variant(vec![case("A", SchemaType::u32())]))],
        )];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("gained a payload"));
    }

    #[test]
    fn variant_payload_removed() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::variant(vec![case("A", SchemaType::u32())]))],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::variant(vec![unit_case("A")]))],
        )];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("lost its payload"));
    }

    #[test]
    fn variant_payload_type_changed() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::variant(vec![case("A", SchemaType::u32())]))],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![(
                "x",
                SchemaType::variant(vec![case("A", SchemaType::string())]),
            )],
        )];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Type changed"));
    }

    #[test]
    fn variant_case_appended_compatible() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::variant(vec![unit_case("A")]))],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![(
                "x",
                SchemaType::variant(vec![unit_case("A"), unit_case("B")]),
            )],
        )];
        assert!(validate_schema_evolution(&old, &new).is_empty());
    }

    #[test]
    fn enum_cases_removed() {
        let old = vec![make_agent_type(
            "A",
            vec![(
                "x",
                SchemaType::r#enum(vec!["A".into(), "B".into(), "C".into()]),
            )],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::r#enum(vec!["A".into(), "B".into()]))],
        )];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Enum cases removed"));
    }

    #[test]
    fn enum_cases_appended_compatible() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::r#enum(vec!["A".into(), "B".into()]))],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![(
                "x",
                SchemaType::r#enum(vec!["A".into(), "B".into(), "C".into()]),
            )],
        )];
        assert!(validate_schema_evolution(&old, &new).is_empty());
    }

    #[test]
    fn flags_removed() {
        let old = vec![make_agent_type(
            "A",
            vec![(
                "x",
                SchemaType::flags(vec!["a".into(), "b".into(), "c".into()]),
            )],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::flags(vec!["a".into(), "b".into()]))],
        )];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Flags removed"));
    }

    #[test]
    fn flags_appended_compatible() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::flags(vec!["a".into(), "b".into()]))],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![(
                "x",
                SchemaType::flags(vec!["a".into(), "b".into(), "c".into()]),
            )],
        )];
        assert!(validate_schema_evolution(&old, &new).is_empty());
    }

    #[test]
    fn tuple_item_removed() {
        let old = vec![make_agent_type(
            "A",
            vec![(
                "x",
                SchemaType::tuple(vec![SchemaType::u32(), SchemaType::string()]),
            )],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::tuple(vec![SchemaType::u32()]))],
        )];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("items removed"));
    }

    #[test]
    fn tuple_append_option_compatible() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::tuple(vec![SchemaType::u32()]))],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![(
                "x",
                SchemaType::tuple(vec![
                    SchemaType::u32(),
                    SchemaType::option(SchemaType::string()),
                ]),
            )],
        )];
        assert!(validate_schema_evolution(&old, &new).is_empty());
    }

    #[test]
    fn tuple_append_non_option_warns() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::tuple(vec![SchemaType::u32()]))],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![(
                "x",
                SchemaType::tuple(vec![SchemaType::u32(), SchemaType::string()]),
            )],
        )];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("non-Option"));
    }

    #[test]
    fn list_inner_type_changed() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::list(SchemaType::u32()))],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::list(SchemaType::string()))],
        )];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Type changed"));
    }

    #[test]
    fn option_inner_type_changed() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::option(SchemaType::u32()))],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![("x", SchemaType::option(SchemaType::string()))],
        )];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Type changed"));
    }

    #[test]
    fn result_ok_type_changed() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", result_ok(SchemaType::u32()))],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![("x", result_ok(SchemaType::string()))],
        )];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Type changed"));
    }

    #[test]
    fn result_err_presence_changed() {
        let old = vec![make_agent_type(
            "A",
            vec![("x", result_ok(SchemaType::u32()))],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![("x", result(SchemaType::u32(), SchemaType::string()))],
        )];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("err payload presence"));
    }

    #[test]
    fn primitive_type_changed() {
        let old = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
        let new = vec![make_agent_type("A", vec![("x", SchemaType::string())])];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Type changed"));
    }

    #[test]
    fn multiple_warnings() {
        let old = vec![make_agent_type(
            "A",
            vec![
                ("x", SchemaType::u32()),
                (
                    "y",
                    rec(vec![("a", SchemaType::string()), ("b", SchemaType::bool())]),
                ),
            ],
        )];
        let new = vec![make_agent_type(
            "A",
            vec![
                ("x", SchemaType::string()),
                ("y", rec(vec![("a", SchemaType::u8())])),
            ],
        )];
        let w = validate_schema_evolution(&old, &new);
        // x: type changed (u32 -> str)
        // y: record fields removed (2 -> 1)
        assert_eq!(w.len(), 2);
    }
}
