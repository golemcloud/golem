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
//! A Scala quantity is `Quantity[U]`, where the marker type `U` carries the
//! unit constraints: the base unit and the suffixes it accepts. A guest marker
//! declares them with the SDK's `@quantityUnit` annotation, an external marker
//! with an implicit runtime `QuantityUnit[U]`. The generated package declares
//! one marker per distinct unit, named after its base unit, so a quantity in
//! kilograms is `Quantity[UnitKg]`.

use crate::bridge_gen::scala::ScalaBridgeMode;
use crate::bridge_gen::scala::scala::unique_idents_with_reserved;
use crate::bridge_gen::scala::scala_writer::ScalaWriter;
use golem_common::schema::AgentTypeSchema;
use golem_common::schema::schema_type::{QuantitySpec, SchemaType};
use heck::ToUpperCamelCase;

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
    pub fn collect(agent: &AgentTypeSchema, reserved: &[&str]) -> Self {
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
        let names = unique_idents_with_reserved(
            specs.iter().map(|(base, _)| marker_name(base)).collect(),
            reserved,
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

    /// Declares the markers and their units.
    pub fn write(&self, w: &mut ScalaWriter, mode: ScalaBridgeMode, client_pkg: &str) {
        for unit in &self.units {
            let marker = format!("{client_pkg}.{}", unit.name);
            let suffixes = unit
                .suffixes
                .iter()
                .map(|s| super::scala_string_literal(s))
                .collect::<Vec<_>>()
                .join(", ");
            match mode {
                ScalaBridgeMode::GuestWasmRpc => {
                    w.line(format!(
                        "@_root_.golem.runtime.annotations.quantityUnit({}) sealed trait {}",
                        annotation_args(&unit.base, &unit.suffixes),
                        unit.name
                    ));
                }
                ScalaBridgeMode::ExternalRest => {
                    let quantity_unit = "_root_.golem.bridge.runtime.QuantityUnit";
                    w.line(format!("sealed trait {}", unit.name));
                    w.line(format!("object {} {{", unit.name));
                    w.indent();
                    w.line(format!(
                        "implicit val quantityUnit: {quantity_unit}[{marker}] = new {quantity_unit}[{marker}] {{"
                    ));
                    w.indent();
                    w.line(format!(
                        "override val baseUnit: _root_.scala.Predef.String = {}",
                        super::scala_string_literal(&unit.base)
                    ));
                    w.line(format!(
                        "override val allowedSuffixes: _root_.scala.collection.immutable.List[_root_.scala.Predef.String] = _root_.scala.collection.immutable.List({suffixes})"
                    ));
                    w.dedent();
                    w.line("}");
                    w.dedent();
                    w.line("}");
                }
            }
            w.blank();
        }
    }
}

/// The `@quantityUnit` arguments that reproduce the spec exactly. The listed
/// form allows every listed unit with the first as the base, so it fits a spec
/// without suffixes or one whose suffixes start with the base; any other spec
/// sets the suffix list with the named `suffixes` argument.
fn annotation_args(base: &str, suffixes: &[String]) -> String {
    let literal = |s: &str| super::scala_string_literal(s);
    if suffixes.is_empty() {
        literal(base)
    } else if suffixes.len() > 1 && suffixes[0] == base {
        suffixes
            .iter()
            .map(|s| literal(s))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        let listed = suffixes
            .iter()
            .map(|s| literal(s))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}, suffixes = _root_.scala.Seq({listed})", literal(base))
    }
}

fn marker_name(base: &str) -> String {
    let ident = base
        .to_upper_camel_case()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>();
    format!("Unit{ident}")
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
