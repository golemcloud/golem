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

//! Unstructured and multimodal content.
//!
//! These are role-marked schema shapes: a variant of an inline case and a url
//! case is unstructured text or binary, and a list of a variant is a
//! multimodal list. A Go client spells them with the shared content types —
//! `values.UnstructuredText[L]`, `values.UnstructuredBinary[M]`,
//! `values.Multimodal` and `values.MultimodalOf[T]` — rather than declaring
//! the variants, so a client reads the same as a Go agent. The language and
//! media-type lists are generated marker types, named after their members.

use crate::bridge_gen::go::go::{go_string, to_exported_ident, unique_idents_with_reserved};
use crate::bridge_gen::go::go_writer::GoWriter;
use golem_common::schema::AgentTypeSchema;
use golem_common::schema::Role;
use golem_common::schema::schema_type::SchemaType;

/// What a role-marked type is.
pub enum Content<'a> {
    /// Unstructured text, with the languages it accepts (empty for any).
    Text(Vec<String>),
    /// Unstructured binary, with the media types it accepts (empty for any).
    Binary(Vec<String>),
    /// A list of text and binary items: `values.Multimodal`.
    BasicMultimodal,
    /// A list of a custom variant's items: `values.MultimodalOf[T]`.
    Multimodal(&'a SchemaType),
}

/// Classifies a resolved type, if it is role-marked content.
pub fn content<'a>(
    resolved: &'a SchemaType,
    resolve: &impl Fn(&'a SchemaType) -> &'a SchemaType,
) -> Option<Content<'a>> {
    match (resolved.metadata().role.as_ref()?, resolved) {
        (Role::UnstructuredText, SchemaType::Variant { cases, .. }) => {
            match cases.first()?.payload.as_ref().map(resolve) {
                Some(SchemaType::Text { restrictions, .. }) => Some(Content::Text(
                    restrictions.languages.clone().unwrap_or_default(),
                )),
                _ => None,
            }
        }
        (Role::UnstructuredBinary, SchemaType::Variant { cases, .. }) => {
            match cases.first()?.payload.as_ref().map(resolve) {
                Some(SchemaType::Binary { restrictions, .. }) => Some(Content::Binary(
                    restrictions.mime_types.clone().unwrap_or_default(),
                )),
                _ => None,
            }
        }
        (Role::Multimodal, SchemaType::List { element, .. }) => {
            if is_basic_modality(resolve(element), resolve) {
                Some(Content::BasicMultimodal)
            } else {
                Some(Content::Multimodal(element))
            }
        }
        _ => None,
    }
}

/// True for the variant of a basic multimodal list: a `Text` case of
/// unstructured text and a `Binary` case of unstructured binary, both
/// unrestricted. The Go SDK spells it as its own `values.Modality`.
pub fn is_basic_modality<'a>(
    resolved: &'a SchemaType,
    resolve: &impl Fn(&'a SchemaType) -> &'a SchemaType,
) -> bool {
    let SchemaType::Variant { cases, .. } = resolved else {
        return false;
    };
    let [text, binary] = cases.as_slice() else {
        return false;
    };
    let payload = |case: &'a golem_common::schema::schema_type::VariantCaseType| {
        case.payload
            .as_ref()
            .and_then(|p| content(resolve(p), resolve))
    };
    text.name == "Text"
        && binary.name == "Binary"
        && matches!(payload(text), Some(Content::Text(l)) if l.is_empty())
        && matches!(payload(binary), Some(Content::Binary(m)) if m.is_empty())
}

/// The language and media-type markers an agent's content needs.
pub(super) struct ContentMarkers {
    markers: Vec<Marker>,
}

struct Marker {
    languages: bool,
    members: Vec<String>,
    name: String,
}

impl ContentMarkers {
    pub fn collect(agent: &AgentTypeSchema, reserved: &[String]) -> Self {
        let mut found: Vec<(bool, Vec<String>)> = Vec::new();
        let mut visit = |typ: &SchemaType| collect(typ, &mut found);
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
            found
                .iter()
                .map(|(languages, members)| {
                    let stem = if *languages { "Languages" } else { "MimeTypes" };
                    let parts = members
                        .iter()
                        .map(|m| to_exported_ident(m))
                        .collect::<String>();
                    format!("{stem}{parts}")
                })
                .collect(),
            &reserved,
        );
        Self {
            markers: found
                .into_iter()
                .zip(names)
                .map(|((languages, members), name)| Marker {
                    languages,
                    members,
                    name,
                })
                .collect(),
        }
    }

    pub fn names(&self) -> impl Iterator<Item = String> + '_ {
        self.markers.iter().map(|m| m.name.clone())
    }

    /// The marker type parameter of a text (`languages`) or binary type.
    pub fn marker(&self, languages: bool, members: &[String]) -> String {
        if members.is_empty() {
            return if languages {
                "values.AnyLanguage".to_string()
            } else {
                "values.AnyMimeType".to_string()
            };
        }
        self.markers
            .iter()
            .find(|m| m.languages == languages && m.members == members)
            .map(|m| m.name.clone())
            .expect("every content restriction has a marker")
    }

    pub fn write(&self, w: &mut GoWriter) {
        for marker in &self.markers {
            let (method, what) = if marker.languages {
                ("Languages", "languages an unstructured text accepts")
            } else {
                ("MimeTypes", "media types an unstructured binary accepts")
            };
            w.doc(&format!(
                "{} lists the {what}: {}.",
                marker.name,
                marker.members.join(", ")
            ));
            w.line(format!("type {} struct{{}}", marker.name));
            w.blank();
            w.line(format!("func ({}) {method}() []string {{", marker.name));
            w.indent();
            let members = marker
                .members
                .iter()
                .map(|m| go_string(m))
                .collect::<Vec<_>>();
            w.line(format!("return []string{{{}}}", members.join(", ")));
            w.dedent();
            w.line("}");
            w.blank();
        }
    }
}

fn collect(typ: &SchemaType, found: &mut Vec<(bool, Vec<String>)>) {
    let mut add = |languages: bool, members: &Option<Vec<String>>| {
        if let Some(members) = members
            && !members.is_empty()
            && !found.contains(&(languages, members.clone()))
        {
            found.push((languages, members.clone()));
        }
    };
    if let Some(role) = typ.metadata().role.as_ref()
        && let SchemaType::Variant { cases, .. } = typ
        && let Some(payload) = cases.first().and_then(|c| c.payload.as_ref())
    {
        match (role, payload) {
            (Role::UnstructuredText, SchemaType::Text { restrictions, .. }) => {
                add(true, &restrictions.languages)
            }
            (Role::UnstructuredBinary, SchemaType::Binary { restrictions, .. }) => {
                add(false, &restrictions.mime_types)
            }
            _ => {}
        }
    }
    let mut recurse = |t: &SchemaType| collect(t, found);
    match typ {
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
        SchemaType::Stream { inner, .. } | SchemaType::Future { inner, .. } => {
            if let Some(inner) = inner {
                recurse(inner);
            }
        }
        _ => {}
    }
}
