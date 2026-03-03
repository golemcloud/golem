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

use crate::base_model::agent::{
    AgentType, AgentTypeName, ComponentModelElementSchema, DataSchema, ElementSchema,
    NamedElementSchemas,
};
use golem_wasm::analysis::AnalysedType;
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
    old_agent_types: &[AgentType],
    new_agent_types: &[AgentType],
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
                validate_data_schema(
                    &old.type_name,
                    "constructor",
                    &old.constructor.input_schema,
                    &new.constructor.input_schema,
                    &mut warnings,
                );
            }
        }
    }

    warnings
}

fn validate_data_schema(
    name: &AgentTypeName,
    path: &str,
    old: &DataSchema,
    new: &DataSchema,
    warnings: &mut Vec<SchemaEvolutionWarning>,
) {
    match (old, new) {
        (DataSchema::Tuple(old_elems), DataSchema::Tuple(new_elems)) => {
            validate_element_schemas(name, path, old_elems, new_elems, warnings);
        }
        (DataSchema::Multimodal(old_elems), DataSchema::Multimodal(new_elems)) => {
            validate_element_schemas(name, path, old_elems, new_elems, warnings);
        }
        _ => {
            warnings.push(SchemaEvolutionWarning {
                agent_type_name: name.clone(),
                path: path.to_string(),
                description: "DataSchema kind changed (Tuple <-> Multimodal)".to_string(),
            });
        }
    }
}

fn validate_element_schemas(
    name: &AgentTypeName,
    path: &str,
    old: &NamedElementSchemas,
    new: &NamedElementSchemas,
    warnings: &mut Vec<SchemaEvolutionWarning>,
) {
    if new.elements.len() < old.elements.len() {
        warnings.push(SchemaEvolutionWarning {
            agent_type_name: name.clone(),
            path: path.to_string(),
            description: format!(
                "Elements removed (was {}, now {})",
                old.elements.len(),
                new.elements.len()
            ),
        });
        return;
    }

    for (i, (old_elem, new_elem)) in old
        .elements
        .iter()
        .zip(new.elements.iter())
        .enumerate()
    {
        let elem_path = format!("{}.elements[{}]", path, i);
        validate_element_schema(name, &elem_path, &old_elem.schema, &new_elem.schema, warnings);
    }
}

fn validate_element_schema(
    name: &AgentTypeName,
    path: &str,
    old: &ElementSchema,
    new: &ElementSchema,
    warnings: &mut Vec<SchemaEvolutionWarning>,
) {
    match (old, new) {
        (
            ElementSchema::ComponentModel(ComponentModelElementSchema {
                element_type: old_type,
            }),
            ElementSchema::ComponentModel(ComponentModelElementSchema {
                element_type: new_type,
            }),
        ) => {
            let type_path = format!("{}.type", path);
            validate_analysed_type(name, &type_path, old_type, new_type, warnings);
        }
        (ElementSchema::UnstructuredText(_), ElementSchema::UnstructuredText(_)) => {}
        (ElementSchema::UnstructuredBinary(_), ElementSchema::UnstructuredBinary(_)) => {}
        _ => {
            warnings.push(SchemaEvolutionWarning {
                agent_type_name: name.clone(),
                path: path.to_string(),
                description: "Element schema kind changed".to_string(),
            });
        }
    }
}

fn discriminant_name(typ: &AnalysedType) -> &'static str {
    match typ {
        AnalysedType::Record(_) => "Record",
        AnalysedType::Variant(_) => "Variant",
        AnalysedType::Enum(_) => "Enum",
        AnalysedType::Flags(_) => "Flags",
        AnalysedType::Tuple(_) => "Tuple",
        AnalysedType::List(_) => "List",
        AnalysedType::Option(_) => "Option",
        AnalysedType::Result(_) => "Result",
        AnalysedType::Str(_) => "Str",
        AnalysedType::Chr(_) => "Chr",
        AnalysedType::F64(_) => "F64",
        AnalysedType::F32(_) => "F32",
        AnalysedType::U64(_) => "U64",
        AnalysedType::S64(_) => "S64",
        AnalysedType::U32(_) => "U32",
        AnalysedType::S32(_) => "S32",
        AnalysedType::U16(_) => "U16",
        AnalysedType::S16(_) => "S16",
        AnalysedType::U8(_) => "U8",
        AnalysedType::S8(_) => "S8",
        AnalysedType::Bool(_) => "Bool",
        AnalysedType::Handle(_) => "Handle",
    }
}

fn validate_analysed_type(
    name: &AgentTypeName,
    path: &str,
    old: &AnalysedType,
    new: &AnalysedType,
    warnings: &mut Vec<SchemaEvolutionWarning>,
) {
    if std::mem::discriminant(old) != std::mem::discriminant(new) {
        warnings.push(SchemaEvolutionWarning {
            agent_type_name: name.clone(),
            path: path.to_string(),
            description: format!(
                "Type changed from {} to {}",
                discriminant_name(old),
                discriminant_name(new)
            ),
        });
        return;
    }

    match (old, new) {
        (AnalysedType::Record(old_rec), AnalysedType::Record(new_rec)) => {
            if new_rec.fields.len() < old_rec.fields.len() {
                warnings.push(SchemaEvolutionWarning {
                    agent_type_name: name.clone(),
                    path: path.to_string(),
                    description: format!(
                        "Record fields removed (was {}, now {})",
                        old_rec.fields.len(),
                        new_rec.fields.len()
                    ),
                });
                return;
            }
            for (i, (old_f, new_f)) in old_rec
                .fields
                .iter()
                .zip(new_rec.fields.iter())
                .enumerate()
            {
                let field_path = format!("{}.fields[{}]", path, i);
                validate_analysed_type(name, &field_path, &old_f.typ, &new_f.typ, warnings);
            }
            for (i, new_f) in new_rec.fields.iter().enumerate().skip(old_rec.fields.len()) {
                if !matches!(new_f.typ, AnalysedType::Option(_)) {
                    let field_path = format!("{}.fields[{}]", path, i);
                    warnings.push(SchemaEvolutionWarning {
                        agent_type_name: name.clone(),
                        path: field_path,
                        description: "Appended non-Option record field".to_string(),
                    });
                }
            }
        }
        (AnalysedType::Tuple(old_tup), AnalysedType::Tuple(new_tup)) => {
            if new_tup.items.len() < old_tup.items.len() {
                warnings.push(SchemaEvolutionWarning {
                    agent_type_name: name.clone(),
                    path: path.to_string(),
                    description: format!(
                        "Tuple items removed (was {}, now {})",
                        old_tup.items.len(),
                        new_tup.items.len()
                    ),
                });
                return;
            }
            for (i, (old_item, new_item)) in
                old_tup.items.iter().zip(new_tup.items.iter()).enumerate()
            {
                let item_path = format!("{}.items[{}]", path, i);
                validate_analysed_type(name, &item_path, old_item, new_item, warnings);
            }
            for (i, new_item) in new_tup.items.iter().enumerate().skip(old_tup.items.len()) {
                if !matches!(new_item, AnalysedType::Option(_)) {
                    let item_path = format!("{}.items[{}]", path, i);
                    warnings.push(SchemaEvolutionWarning {
                        agent_type_name: name.clone(),
                        path: item_path,
                        description: "Appended non-Option tuple item".to_string(),
                    });
                }
            }
        }
        (AnalysedType::Variant(old_var), AnalysedType::Variant(new_var)) => {
            if new_var.cases.len() < old_var.cases.len() {
                warnings.push(SchemaEvolutionWarning {
                    agent_type_name: name.clone(),
                    path: path.to_string(),
                    description: format!(
                        "Variant cases removed (was {}, now {})",
                        old_var.cases.len(),
                        new_var.cases.len()
                    ),
                });
                return;
            }
            for (i, (old_case, new_case)) in
                old_var.cases.iter().zip(new_var.cases.iter()).enumerate()
            {
                let case_path = format!("{}.cases[{}]", path, i);
                match (&old_case.typ, &new_case.typ) {
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
                        validate_analysed_type(name, &case_path, old_t, new_t, warnings);
                    }
                    (None, None) => {}
                }
            }
        }
        (AnalysedType::Enum(old_enum), AnalysedType::Enum(new_enum)) => {
            if new_enum.cases.len() < old_enum.cases.len() {
                warnings.push(SchemaEvolutionWarning {
                    agent_type_name: name.clone(),
                    path: path.to_string(),
                    description: format!(
                        "Enum cases removed (was {}, now {})",
                        old_enum.cases.len(),
                        new_enum.cases.len()
                    ),
                });
            }
        }
        (AnalysedType::Flags(old_flags), AnalysedType::Flags(new_flags)) => {
            if new_flags.names.len() < old_flags.names.len() {
                warnings.push(SchemaEvolutionWarning {
                    agent_type_name: name.clone(),
                    path: path.to_string(),
                    description: format!(
                        "Flags removed (was {}, now {})",
                        old_flags.names.len(),
                        new_flags.names.len()
                    ),
                });
            }
        }
        (AnalysedType::List(old_list), AnalysedType::List(new_list)) => {
            validate_analysed_type(name, path, &old_list.inner, &new_list.inner, warnings);
        }
        (AnalysedType::Option(old_opt), AnalysedType::Option(new_opt)) => {
            validate_analysed_type(name, path, &old_opt.inner, &new_opt.inner, warnings);
        }
        (AnalysedType::Result(old_res), AnalysedType::Result(new_res)) => {
            match (&old_res.ok, &new_res.ok) {
                (None, Some(_)) | (Some(_), None) => {
                    warnings.push(SchemaEvolutionWarning {
                        agent_type_name: name.clone(),
                        path: format!("{}.ok", path),
                        description: "Result ok payload presence changed".to_string(),
                    });
                }
                (Some(old_ok), Some(new_ok)) => {
                    validate_analysed_type(
                        name,
                        &format!("{}.ok", path),
                        old_ok,
                        new_ok,
                        warnings,
                    );
                }
                (None, None) => {}
            }
            match (&old_res.err, &new_res.err) {
                (None, Some(_)) | (Some(_), None) => {
                    warnings.push(SchemaEvolutionWarning {
                        agent_type_name: name.clone(),
                        path: format!("{}.err", path),
                        description: "Result err payload presence changed".to_string(),
                    });
                }
                (Some(old_err), Some(new_err)) => {
                    validate_analysed_type(
                        name,
                        &format!("{}.err", path),
                        old_err,
                        new_err,
                        warnings,
                    );
                }
                (None, None) => {}
            }
        }
        // Primitives with same discriminant: compatible
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_model::agent::{
        AgentConstructor, AgentMode, AgentTypeName, DataSchema, NamedElementSchema,
        NamedElementSchemas, Snapshotting,
    };
    use crate::model::Empty;
    use golem_wasm::analysis::analysed_type::{
        bool, case, field, flags, list, option, r#enum, record, result, result_err, result_ok, s32,
        str, tuple, u32, u8, unit_case, variant,
    };
    use test_r::test;

    fn cm_schema(typ: AnalysedType) -> ElementSchema {
        ElementSchema::ComponentModel(ComponentModelElementSchema { element_type: typ })
    }

    fn tuple_data_schema(elements: Vec<(&str, ElementSchema)>) -> DataSchema {
        DataSchema::Tuple(NamedElementSchemas {
            elements: elements
                .into_iter()
                .map(|(name, schema)| NamedElementSchema {
                    name: name.to_string(),
                    schema,
                })
                .collect(),
        })
    }

    fn make_agent_type(name: &str, schema: DataSchema) -> AgentType {
        AgentType {
            type_name: AgentTypeName(name.to_string()),
            description: String::new(),
            source_language: String::new(),
            constructor: AgentConstructor {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: schema,
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
        let schema = tuple_data_schema(vec![("x", cm_schema(u32()))]);
        let old = vec![make_agent_type("A", schema.clone())];
        let new = vec![make_agent_type("A", schema)];
        assert!(validate_schema_evolution(&old, &new).is_empty());
    }

    #[test]
    fn agent_type_removed() {
        let schema = tuple_data_schema(vec![("x", cm_schema(u32()))]);
        let old = vec![make_agent_type("A", schema)];
        let new = vec![];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("removed"));
    }

    #[test]
    fn agent_type_added_no_warning() {
        let schema = tuple_data_schema(vec![("x", cm_schema(u32()))]);
        let old = vec![];
        let new = vec![make_agent_type("A", schema)];
        assert!(validate_schema_evolution(&old, &new).is_empty());
    }

    #[test]
    fn data_schema_kind_changed() {
        let old_schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![],
        });
        let new_schema = DataSchema::Multimodal(NamedElementSchemas {
            elements: vec![],
        });
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("kind changed"));
    }

    #[test]
    fn element_removed() {
        let old_schema = tuple_data_schema(vec![("x", cm_schema(u32())), ("y", cm_schema(str()))]);
        let new_schema = tuple_data_schema(vec![("x", cm_schema(u32()))]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("removed"));
    }

    #[test]
    fn element_schema_kind_changed() {
        use crate::base_model::agent::TextDescriptor;
        let old_schema = tuple_data_schema(vec![("x", cm_schema(u32()))]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            ElementSchema::UnstructuredText(TextDescriptor {
                restrictions: None,
            }),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("kind changed"));
    }

    #[test]
    fn record_field_removed() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(record(vec![field("a", u32()), field("b", str())])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(record(vec![field("a", u32())])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("fields removed"));
    }

    #[test]
    fn record_field_type_changed() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(record(vec![field("a", u32())])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(record(vec![field("a", str())])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Type changed"));
    }

    #[test]
    fn record_append_option_field_compatible() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(record(vec![field("a", u32())])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(record(vec![field("a", u32()), field("b", option(str()))])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        assert!(validate_schema_evolution(&old, &new).is_empty());
    }

    #[test]
    fn record_append_non_option_field_warns() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(record(vec![field("a", u32())])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(record(vec![field("a", u32()), field("b", str())])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("non-Option"));
    }

    #[test]
    fn variant_case_removed() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(variant(vec![unit_case("A"), unit_case("B")])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(variant(vec![unit_case("A")])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("cases removed"));
    }

    #[test]
    fn variant_payload_added() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(variant(vec![unit_case("A")])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(variant(vec![case("A", u32())])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("gained a payload"));
    }

    #[test]
    fn variant_payload_removed() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(variant(vec![case("A", u32())])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(variant(vec![unit_case("A")])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("lost its payload"));
    }

    #[test]
    fn variant_payload_type_changed() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(variant(vec![case("A", u32())])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(variant(vec![case("A", str())])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Type changed"));
    }

    #[test]
    fn variant_case_appended_compatible() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(variant(vec![unit_case("A")])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(variant(vec![unit_case("A"), unit_case("B")])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        assert!(validate_schema_evolution(&old, &new).is_empty());
    }

    #[test]
    fn enum_cases_removed() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(r#enum(&["A", "B", "C"])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(r#enum(&["A", "B"])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Enum cases removed"));
    }

    #[test]
    fn enum_cases_appended_compatible() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(r#enum(&["A", "B"])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(r#enum(&["A", "B", "C"])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        assert!(validate_schema_evolution(&old, &new).is_empty());
    }

    #[test]
    fn flags_removed() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(flags(&["a", "b", "c"])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(flags(&["a", "b"])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Flags removed"));
    }

    #[test]
    fn flags_appended_compatible() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(flags(&["a", "b"])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(flags(&["a", "b", "c"])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        assert!(validate_schema_evolution(&old, &new).is_empty());
    }

    #[test]
    fn tuple_item_removed() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(tuple(vec![u32(), str()])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(tuple(vec![u32()])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("items removed"));
    }

    #[test]
    fn tuple_append_option_compatible() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(tuple(vec![u32()])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(tuple(vec![u32(), option(str())])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        assert!(validate_schema_evolution(&old, &new).is_empty());
    }

    #[test]
    fn tuple_append_non_option_warns() {
        let old_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(tuple(vec![u32()])),
        )]);
        let new_schema = tuple_data_schema(vec![(
            "x",
            cm_schema(tuple(vec![u32(), str()])),
        )]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("non-Option"));
    }

    #[test]
    fn list_inner_type_changed() {
        let old_schema = tuple_data_schema(vec![("x", cm_schema(list(u32())))]);
        let new_schema = tuple_data_schema(vec![("x", cm_schema(list(str())))]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Type changed"));
    }

    #[test]
    fn option_inner_type_changed() {
        let old_schema = tuple_data_schema(vec![("x", cm_schema(option(u32())))]);
        let new_schema = tuple_data_schema(vec![("x", cm_schema(option(str())))]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Type changed"));
    }

    #[test]
    fn result_ok_type_changed() {
        let old_schema = tuple_data_schema(vec![("x", cm_schema(result_ok(u32())))]);
        let new_schema = tuple_data_schema(vec![("x", cm_schema(result_ok(str())))]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Type changed"));
    }

    #[test]
    fn result_err_presence_changed() {
        let old_schema = tuple_data_schema(vec![("x", cm_schema(result_ok(u32())))]);
        let new_schema = tuple_data_schema(vec![("x", cm_schema(result(u32(), str())))]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("err payload presence"));
    }

    #[test]
    fn primitive_type_changed() {
        let old_schema = tuple_data_schema(vec![("x", cm_schema(u32()))]);
        let new_schema = tuple_data_schema(vec![("x", cm_schema(str()))]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        assert_eq!(w.len(), 1);
        assert!(w[0].description.contains("Type changed"));
    }

    #[test]
    fn multiple_warnings() {
        let old_schema = tuple_data_schema(vec![
            ("x", cm_schema(u32())),
            ("y", cm_schema(record(vec![field("a", str()), field("b", bool())]))),
        ]);
        let new_schema = tuple_data_schema(vec![
            ("x", cm_schema(str())),
            ("y", cm_schema(record(vec![field("a", u8())]))),
        ]);
        let old = vec![make_agent_type("A", old_schema)];
        let new = vec![make_agent_type("A", new_schema)];
        let w = validate_schema_evolution(&old, &new);
        // x: type changed (u32 -> str)
        // y: record fields removed (2 -> 1)
        assert_eq!(w.len(), 2);
    }
}
