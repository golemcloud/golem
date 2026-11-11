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

//! Constructor schema evolution of agent types.
//!
//! A keyed agent's key is its user-supplied constructor parameters in the canonical
//! agent key text (see [`super::structural_format`]). A constructor change is
//! compatible only when the canonical text of every existing key still parses with
//! the new schema and formats to the same text; any other change gives existing
//! keyed agents new keys. This module reports the changes that are not compatible.

use crate::base_model::agent::AgentTypeName;
use crate::schema::agent::AgentTypeSchema;
use crate::schema::graph::{SchemaGraph, reachable_defs};
use crate::schema::schema_type::SchemaType;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct SchemaEvolutionError {
    pub agent_type_name: AgentTypeName,
    pub path: String,
    pub description: String,
}

impl fmt::Display for SchemaEvolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Agent '{}' at {}: {}",
            self.agent_type_name, self.path, self.description
        )
    }
}

/// The schema of an agent type's keys: its user-supplied constructor parameter record,
/// with the definitions reachable from it.
pub fn constructor_key_schema(agent_type: &AgentTypeSchema) -> SchemaGraph {
    let root = super::constructor_parameter_record(agent_type);
    let defs = reachable_defs(&agent_type.schema, &root);
    SchemaGraph { defs, root }
}

/// Compares two key schemas (see [`constructor_key_schema`]) of one agent type and
/// returns every change that would change the key of an existing agent.
pub fn validate_constructor_evolution(
    agent_type_name: &AgentTypeName,
    old: &SchemaGraph,
    new: &SchemaGraph,
) -> Vec<SchemaEvolutionError> {
    let mut checker = Checker {
        agent_type_name,
        old_graph: old,
        new_graph: new,
        visited: Vec::new(),
        errors: Vec::new(),
    };
    checker.check("constructor", &old.root, &new.root);
    checker.errors
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
        SchemaType::Uuid { .. } => "Uuid",
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

struct Checker<'a> {
    agent_type_name: &'a AgentTypeName,
    old_graph: &'a SchemaGraph,
    new_graph: &'a SchemaGraph,
    /// Pairs of nodes already compared where one side is a reference, so recursive
    /// types terminate.
    visited: Vec<(*const SchemaType, *const SchemaType)>,
    errors: Vec<SchemaEvolutionError>,
}

impl<'a> Checker<'a> {
    fn error(&mut self, path: &str, description: impl Into<String>) {
        self.errors.push(SchemaEvolutionError {
            agent_type_name: self.agent_type_name.clone(),
            path: path.to_string(),
            description: description.into(),
        });
    }

    fn check(&mut self, path: &str, old: &'a SchemaType, new: &'a SchemaType) {
        if matches!(old, SchemaType::Ref { .. }) || matches!(new, SchemaType::Ref { .. }) {
            let pair = (old as *const SchemaType, new as *const SchemaType);
            if self.visited.contains(&pair) {
                return;
            }
            self.visited.push(pair);
        }
        let old = resolve(self.old_graph, old);
        let new = resolve(self.new_graph, new);

        let old_disc = discriminant_name(old);
        let new_disc = discriminant_name(new);
        if old_disc != new_disc {
            self.error(path, format!("Type changed from {old_disc} to {new_disc}"));
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
                    self.error(
                        path,
                        format!(
                            "Record fields removed (was {}, now {})",
                            old_fields.len(),
                            new_fields.len()
                        ),
                    );
                    return;
                }
                for (i, (old_f, new_f)) in old_fields.iter().zip(new_fields).enumerate() {
                    self.check(&format!("{path}.fields[{i}]"), &old_f.body, &new_f.body);
                }
                for (i, new_f) in new_fields.iter().enumerate().skip(old_fields.len()) {
                    if !is_option(self.new_graph, &new_f.body) {
                        self.error(
                            &format!("{path}.fields[{i}]"),
                            "Appended non-Option record field",
                        );
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
                if new_items.len() != old_items.len() {
                    let change = if new_items.len() < old_items.len() {
                        "removed"
                    } else {
                        "added"
                    };
                    self.error(
                        path,
                        format!(
                            "Tuple items {change} (was {}, now {})",
                            old_items.len(),
                            new_items.len()
                        ),
                    );
                    return;
                }
                for (i, (old_item, new_item)) in old_items.iter().zip(new_items).enumerate() {
                    self.check(&format!("{path}.items[{i}]"), old_item, new_item);
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
                    self.error(
                        path,
                        format!(
                            "Variant cases removed (was {}, now {})",
                            old_cases.len(),
                            new_cases.len()
                        ),
                    );
                    return;
                }
                for (i, (old_case, new_case)) in old_cases.iter().zip(new_cases).enumerate() {
                    let case_path = format!("{path}.cases[{i}]");
                    match (&old_case.payload, &new_case.payload) {
                        (None, Some(_)) => self.error(&case_path, "Variant case gained a payload"),
                        (Some(_), None) => self.error(&case_path, "Variant case lost its payload"),
                        (Some(old_t), Some(new_t)) => self.check(&case_path, old_t, new_t),
                        (None, None) => {}
                    }
                }
            }
            (
                SchemaType::Union { spec: old_spec, .. },
                SchemaType::Union { spec: new_spec, .. },
            ) => {
                if new_spec.branches.len() < old_spec.branches.len() {
                    self.error(
                        path,
                        format!(
                            "Union branches removed (was {}, now {})",
                            old_spec.branches.len(),
                            new_spec.branches.len()
                        ),
                    );
                    return;
                }
                for (i, (old_branch, new_branch)) in
                    old_spec.branches.iter().zip(&new_spec.branches).enumerate()
                {
                    self.check(
                        &format!("{path}.branches[{i}]"),
                        &old_branch.body,
                        &new_branch.body,
                    );
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
                self.error(
                    path,
                    format!(
                        "Enum cases removed (was {}, now {})",
                        old_cases.len(),
                        new_cases.len()
                    ),
                );
            }
            (
                SchemaType::Flags {
                    flags: old_flags, ..
                },
                SchemaType::Flags {
                    flags: new_flags, ..
                },
            ) if new_flags.len() < old_flags.len() => {
                self.error(
                    path,
                    format!(
                        "Flags removed (was {}, now {})",
                        old_flags.len(),
                        new_flags.len()
                    ),
                );
            }
            (
                SchemaType::List {
                    element: old_inner, ..
                },
                SchemaType::List {
                    element: new_inner, ..
                },
            ) => self.check(path, old_inner, new_inner),
            (
                SchemaType::FixedList {
                    element: old_inner,
                    length: old_length,
                    ..
                },
                SchemaType::FixedList {
                    element: new_inner,
                    length: new_length,
                    ..
                },
            ) => {
                if old_length != new_length {
                    self.error(
                        path,
                        format!("Fixed list length changed from {old_length} to {new_length}"),
                    );
                    return;
                }
                self.check(path, old_inner, new_inner);
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
                self.check(&format!("{path}.key"), old_key, new_key);
                self.check(&format!("{path}.value"), old_value, new_value);
            }
            (
                SchemaType::Option {
                    inner: old_inner, ..
                },
                SchemaType::Option {
                    inner: new_inner, ..
                },
            ) => self.check(path, old_inner, new_inner),
            (
                SchemaType::Result { spec: old_spec, .. },
                SchemaType::Result { spec: new_spec, .. },
            ) => {
                for (arm, old_arm, new_arm) in [
                    ("ok", &old_spec.ok, &new_spec.ok),
                    ("err", &old_spec.err, &new_spec.err),
                ] {
                    let arm_path = format!("{path}.{arm}");
                    match (old_arm, new_arm) {
                        (None, Some(_)) | (Some(_), None) => {
                            self.error(&arm_path, format!("Result {arm} payload presence changed"))
                        }
                        (Some(old_t), Some(new_t)) => self.check(&arm_path, old_t, new_t),
                        (None, None) => {}
                    }
                }
            }
            // Same-discriminant primitives, rich scalars, capabilities and P3
            // stubs keep their text.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
