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

//! Phantom markers of the rich `text` and `quantity` kinds.
//!
//! A bare text is `Text[S]` and a quantity is `Quantity[U]`, as in the MoonBit
//! SDK. The marker type carries the schema's constraints: `S` implements
//! `TextSpec` with the text restrictions, and `U` implements `QuantityUnit`
//! with the base unit and the allowed suffixes. The generated package declares
//! one marker per distinct constraint set.

use crate::bridge_gen::moonbit::mbt_writer::MoonBitWriter;
use crate::bridge_gen::moonbit::moonbit::unique_idents_with_reserved;
use golem_common::schema::schema_type::{QuantitySpec, SchemaType, TextRestrictions};
use golem_common::schema::unstructured::{
    unstructured_binary_restrictions, unstructured_text_restrictions,
};
use golem_common::schema::{AgentTypeSchema, SchemaGraph};
use heck::ToUpperCamelCase;

pub(super) struct RichMarkers {
    units: Vec<(String, Vec<String>, String)>,
    texts: Vec<(TextRestrictions, String)>,
}

impl RichMarkers {
    /// Every distinct quantity unit and text restriction set of the agent,
    /// named clear of `used`.
    pub fn collect(agent: &AgentTypeSchema, graph: &SchemaGraph, used: &[String]) -> Self {
        let mut specs = Specs::default();
        let mut visit = |typ: &SchemaType| specs.visit(graph, typ);
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
        for def in &graph.defs {
            visit(&def.body);
        }

        let reserved = used.iter().map(String::as_str).collect::<Vec<_>>();
        let mut candidates = specs
            .units
            .iter()
            .enumerate()
            .map(|(idx, (base, _))| {
                let ident = base
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
                    .collect::<String>()
                    .to_upper_camel_case();
                if ident.is_empty() {
                    format!("Unit{idx}")
                } else {
                    format!("Unit{ident}")
                }
            })
            .collect::<Vec<_>>();
        candidates.extend((0..specs.texts.len()).map(|idx| format!("TextSpec{idx}")));
        let mut names = unique_idents_with_reserved(candidates, &reserved).into_iter();

        Self {
            units: specs
                .units
                .into_iter()
                .map(|(base, suffixes)| (base, suffixes, names.next().unwrap()))
                .collect(),
            texts: specs
                .texts
                .into_iter()
                .map(|restrictions| (restrictions, names.next().unwrap()))
                .collect(),
        }
    }

    pub fn unit_name(&self, spec: &QuantitySpec) -> anyhow::Result<&str> {
        self.units
            .iter()
            .find(|(base, suffixes, _)| {
                *base == spec.base_unit && *suffixes == spec.allowed_suffixes
            })
            .map(|(_, _, name)| name.as_str())
            .ok_or_else(|| anyhow::anyhow!("No MoonBit quantity unit marker for {spec:?}"))
    }

    pub fn text_name(&self, restrictions: &TextRestrictions) -> anyhow::Result<&str> {
        self.texts
            .iter()
            .find(|(r, _)| r == restrictions)
            .map(|(_, name)| name.as_str())
            .ok_or_else(|| anyhow::anyhow!("No MoonBit text marker for {restrictions:?}"))
    }

    /// Declares the markers against the traits of `package` (`@schema` in
    /// guest mode, `@runtime` in external mode).
    pub fn write(&self, writer: &mut MoonBitWriter, package: &str, module: &str) {
        for (base, suffixes, name) in &self.units {
            writer.doc(&format!("Unit marker of quantities measured in `{base}`."));
            write_marker(writer, name);
            let trait_name = format!("{package}.QuantityUnit");
            write_method(writer, &trait_name, name, "type_id", &type_id(module, name));
            write_method(
                writer,
                &trait_name,
                name,
                "base_unit",
                &string_literal(base),
            );
            write_method(
                writer,
                &trait_name,
                name,
                "allowed_suffixes",
                &string_array(suffixes),
            );
        }
        for (restrictions, name) in &self.texts {
            writer.doc("Restriction marker of a text kind.");
            write_marker(writer, name);
            let trait_name = format!("{package}.TextSpec");
            write_method(writer, &trait_name, name, "type_id", &type_id(module, name));
            write_method(
                writer,
                &trait_name,
                name,
                "languages",
                &string_array(restrictions.languages.as_deref().unwrap_or_default()),
            );
            write_method(
                writer,
                &trait_name,
                name,
                "min_length",
                &opt_uint(restrictions.min_length),
            );
            write_method(
                writer,
                &trait_name,
                name,
                "max_length",
                &opt_uint(restrictions.max_length),
            );
            write_method(
                writer,
                &trait_name,
                name,
                "regex",
                &match &restrictions.regex {
                    Some(regex) => format!("Some({})", string_literal(regex)),
                    None => "None".to_string(),
                },
            );
        }
    }
}

#[derive(Default)]
struct Specs {
    units: Vec<(String, Vec<String>)>,
    texts: Vec<TextRestrictions>,
}

impl Specs {
    fn visit(&mut self, graph: &SchemaGraph, typ: &SchemaType) {
        if matches!(unstructured_text_restrictions(graph, typ), Ok(Some(_)))
            || matches!(unstructured_binary_restrictions(graph, typ), Ok(Some(_)))
        {
            return;
        }
        match typ {
            SchemaType::Quantity { spec, .. } => {
                let key = (spec.base_unit.clone(), spec.allowed_suffixes.clone());
                if !self.units.contains(&key) {
                    self.units.push(key);
                }
            }
            SchemaType::Text { restrictions, .. } => {
                if !self.texts.contains(restrictions) {
                    self.texts.push(restrictions.clone());
                }
            }
            SchemaType::Record { fields, .. } => {
                fields.iter().for_each(|f| self.visit(graph, &f.body))
            }
            SchemaType::Variant { cases, .. } => cases
                .iter()
                .filter_map(|c| c.payload.as_ref())
                .for_each(|t| self.visit(graph, t)),
            SchemaType::Tuple { elements, .. } => {
                elements.iter().for_each(|t| self.visit(graph, t))
            }
            SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
                self.visit(graph, element)
            }
            SchemaType::Map { key, value, .. } => {
                self.visit(graph, key);
                self.visit(graph, value);
            }
            SchemaType::Option { inner, .. } => self.visit(graph, inner),
            SchemaType::Result { spec, .. } => {
                if let Some(ok) = spec.ok.as_deref() {
                    self.visit(graph, ok);
                }
                if let Some(err) = spec.err.as_deref() {
                    self.visit(graph, err);
                }
            }
            SchemaType::Union { spec, .. } => spec
                .branches
                .iter()
                .for_each(|b| self.visit(graph, &b.body)),
            SchemaType::Secret { spec, .. } => self.visit(graph, &spec.inner),
            SchemaType::Stream { inner, .. } | SchemaType::Future { inner, .. } => {
                if let Some(inner) = inner {
                    self.visit(graph, inner);
                }
            }
            _ => {}
        }
    }
}

fn write_marker(writer: &mut MoonBitWriter, name: &str) {
    writer.line(format!("pub(all) struct {name} {{}} derive(Debug, Eq)"));
    writer.blank();
}

fn write_method(
    writer: &mut MoonBitWriter,
    trait_name: &str,
    name: &str,
    method: &str,
    body: &str,
) {
    writer.line(format!(
        "pub impl {trait_name} for {name} with fn {method}() {{"
    ));
    writer.indent();
    writer.line(body);
    writer.dedent();
    writer.line("}");
    writer.blank();
}

fn type_id(module: &str, name: &str) -> String {
    string_literal(&format!("{module}.{name}"))
}

fn string_literal(value: &str) -> String {
    super::moonbit_string_literal(value)
}

fn string_array(values: &[String]) -> String {
    super::moonbit_string_array(Some(values))
}

fn opt_uint(value: Option<u32>) -> String {
    match value {
        Some(v) => format!("Some({v}U)"),
        None => "None".to_string(),
    }
}
