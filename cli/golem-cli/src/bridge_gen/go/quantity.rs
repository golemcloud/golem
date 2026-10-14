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

//! Quantity unit markers.
//!
//! A Go quantity is `values.Quantity[U]`, where the marker type `U` carries the
//! unit constraints: its base unit and the suffixes it also accepts. The
//! generated package declares one marker per distinct unit, named after its
//! base unit, so a quantity in kilograms is `values.Quantity[UnitKg]`.

use crate::bridge_gen::go::go::{go_string, to_exported_ident, unique_idents_with_reserved};
use crate::bridge_gen::go::go_writer::GoWriter;
use golem_common::schema::AgentTypeSchema;
use golem_common::schema::schema_type::{QuantitySpec, SchemaType};

pub(super) struct QuantityUnits {
    units: Vec<Unit>,
}

struct Unit {
    base: String,
    suffixes: Vec<String>,
    name: String,
}

impl QuantityUnits {
    /// Every distinct unit the agent's schema uses, named clear of `reserved`.
    pub fn collect(agent: &AgentTypeSchema, reserved: &[String]) -> Self {
        let mut specs: Vec<(String, Vec<String>)> = Vec::new();
        let mut visit = |typ: &SchemaType| collect_specs(typ, &mut specs);
        for field in agent.constructor.input_schema.fields() {
            visit(&field.schema);
        }
        for method in &agent.methods {
            for field in method.input_schema.fields() {
                visit(&field.schema);
            }
            if let Some(output) = method.output_schema.schema() {
                visit(output);
            }
        }
        for config in &agent.config {
            visit(&config.value_type);
        }
        for def in &agent.schema.defs {
            visit(&def.body);
        }
        let reserved = reserved.iter().map(String::as_str).collect::<Vec<_>>();
        let names = unique_idents_with_reserved(
            specs
                .iter()
                .map(|(base, _)| format!("Unit{}", to_exported_ident(base)))
                .collect(),
            &reserved,
        );
        Self {
            units: specs
                .into_iter()
                .zip(names)
                .map(|((base, suffixes), name)| Unit {
                    base,
                    suffixes,
                    name,
                })
                .collect(),
        }
    }

    pub fn names(&self) -> impl Iterator<Item = String> + '_ {
        self.units.iter().map(|u| u.name.clone())
    }

    pub fn name_for(&self, spec: &QuantitySpec) -> Option<&str> {
        self.units
            .iter()
            .find(|u| u.base == spec.base_unit && u.suffixes == spec.allowed_suffixes)
            .map(|u| u.name.as_str())
    }

    /// Declares the markers.
    pub fn write(&self, w: &mut GoWriter) {
        for unit in &self.units {
            w.doc(&format!(
                "{} is the unit of quantities measured in {}.",
                unit.name, unit.base
            ));
            w.line(format!("type {} struct{{}}", unit.name));
            w.blank();
            w.line(format!("func ({}) BaseUnit() string {{", unit.name));
            w.indent();
            w.line(format!("return {}", go_string(&unit.base)));
            w.dedent();
            w.line("}");
            w.blank();
            w.line(format!(
                "func ({}) AllowedSuffixes() []string {{",
                unit.name
            ));
            w.indent();
            if unit.suffixes.is_empty() {
                w.line("return nil");
            } else {
                let suffixes = unit
                    .suffixes
                    .iter()
                    .map(|s| go_string(s))
                    .collect::<Vec<_>>();
                w.line(format!("return []string{{{}}}", suffixes.join(", ")));
            }
            w.dedent();
            w.line("}");
            w.blank();
        }
    }
}

fn collect_specs(typ: &SchemaType, specs: &mut Vec<(String, Vec<String>)>) {
    let mut recurse = |t: &SchemaType| collect_specs(t, specs);
    match typ {
        SchemaType::Quantity { spec, .. } => {
            let key = (spec.base_unit.clone(), spec.allowed_suffixes.clone());
            if !specs.contains(&key) {
                specs.push(key);
            }
        }
        SchemaType::Record { fields, .. } => fields.iter().for_each(|f| recurse(&f.body)),
        SchemaType::Variant { cases, .. } => cases
            .iter()
            .filter_map(|c| c.payload.as_ref())
            .for_each(&mut recurse),
        SchemaType::Tuple { elements, .. } => elements.iter().for_each(recurse),
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            recurse(element)
        }
        SchemaType::Map { key, value, .. } => {
            recurse(key);
            recurse(value);
        }
        SchemaType::Option { inner, .. } => recurse(inner),
        SchemaType::Result { spec, .. } => {
            if let Some(ok) = spec.ok.as_deref() {
                recurse(ok);
            }
            if let Some(err) = spec.err.as_deref() {
                recurse(err);
            }
        }
        SchemaType::Union { spec, .. } => spec.branches.iter().for_each(|b| recurse(&b.body)),
        SchemaType::Secret { spec, .. } => recurse(&spec.inner),
        SchemaType::Stream { inner, .. } | SchemaType::Future { inner, .. } => {
            if let Some(inner) = inner {
                recurse(inner);
            }
        }
        _ => {}
    }
}
