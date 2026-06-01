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

//! Markdown documentation emitter for a [`SchemaGraph`].
//!
//! Each named definition becomes a `##` section; the root is added under
//! the caller-supplied name. When the root is a [`SchemaType::Ref`] the
//! ref is inlined so the root section carries the referenced body's
//! fields/cases/branches directly instead of a bare ref pointer.
//!
//! Each section carries:
//! - the metadata `doc` paragraph (if any),
//! - a fenced code block with the CLI-text rendering of the body,
//! - `### Fields` / `### Cases` / `### Branches` lists when the body is a
//!   record / variant / union / enum / flags (per-field/case/branch doc
//!   and examples are emitted inline),
//! - an `### Examples` block listing every metadata `examples` entry as
//!   its own fenced JSON block.

use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::MetadataEnvelope;
use crate::schema::render::cli_text::type_to_cli_text;
use crate::schema::schema_type::{
    DiscriminatorRule, SchemaType, UnionBranch, UnionSpec, VariantCaseType,
};
use std::fmt::Write;

/// Emit a Markdown document describing every named definition in the
/// graph plus the root (under `root_name`).
pub fn graph_to_markdown(graph: &SchemaGraph, root_name: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Schema");
    let _ = writeln!(out);

    // Inline the body of a root `Ref(id)` so the root section shows the
    // structure instead of just the pointer. Metadata now lives on the
    // body's own envelope (def has no separate metadata slot).
    let (root_body, root_metadata): (&SchemaType, MetadataEnvelope) = match &graph.root {
        SchemaType::Ref { id, .. } => match graph.lookup(id) {
            Some(def) => (&def.body, def.body.metadata().clone()),
            None => (&graph.root, graph.root.metadata().clone()),
        },
        other => (other, other.metadata().clone()),
    };
    section(&mut out, root_name, &root_metadata, graph, root_body);

    for def in &graph.defs {
        let title = def.name.clone().unwrap_or_else(|| def.id.0.clone());
        section(&mut out, &title, def.body.metadata(), graph, &def.body);
    }

    out
}

fn section(
    out: &mut String,
    title: &str,
    metadata: &MetadataEnvelope,
    graph: &SchemaGraph,
    body: &SchemaType,
) {
    let _ = writeln!(out, "## {title}");
    let _ = writeln!(out);
    if let Some(doc) = &metadata.doc {
        let _ = writeln!(out, "{doc}");
        let _ = writeln!(out);
    }
    let _ = writeln!(out, "```");
    let _ = writeln!(out, "{}", type_to_cli_text(graph, body));
    let _ = writeln!(out, "```");
    let _ = writeln!(out);

    match body {
        SchemaType::Record { fields, .. } => {
            let _ = writeln!(out, "### Fields");
            let _ = writeln!(out);
            for field in fields {
                let _ = writeln!(
                    out,
                    "- `{}`: `{}`",
                    field.name,
                    type_to_cli_text(graph, &field.body)
                );
                // Prefer the field-level metadata doc; fall back to the
                // inline body's metadata so inline-typed fields can still
                // attach documentation.
                let field_doc = field
                    .metadata
                    .doc
                    .as_ref()
                    .or(field.body.metadata().doc.as_ref());
                if let Some(doc) = field_doc {
                    let _ = writeln!(out, "  - {doc}");
                }
                write_inline_examples(out, &field.metadata);
                write_inline_examples(out, field.body.metadata());
            }
            let _ = writeln!(out);
        }
        SchemaType::Variant { cases, .. } => {
            let _ = writeln!(out, "### Cases");
            let _ = writeln!(out);
            for case in cases {
                write_variant_case(out, graph, case);
            }
            let _ = writeln!(out);
        }
        SchemaType::Enum { cases, .. } => {
            let _ = writeln!(out, "### Cases");
            let _ = writeln!(out);
            for case in cases {
                let _ = writeln!(out, "- `{case}`");
            }
            let _ = writeln!(out);
        }
        SchemaType::Flags { flags, .. } => {
            let _ = writeln!(out, "### Flags");
            let _ = writeln!(out);
            for flag in flags {
                let _ = writeln!(out, "- `{flag}`");
            }
            let _ = writeln!(out);
        }
        SchemaType::Union { spec, .. } => {
            write_union_branches(out, graph, spec);
        }
        _ => {}
    }

    if !metadata.examples.is_empty() {
        let _ = writeln!(out, "### Examples");
        let _ = writeln!(out);
        for example in &metadata.examples {
            let _ = writeln!(out, "```json");
            let _ = writeln!(out, "{example}");
            let _ = writeln!(out, "```");
            let _ = writeln!(out);
        }
    }

    if !metadata.aliases.is_empty() {
        let _ = writeln!(out, "### Aliases");
        let _ = writeln!(out);
        for alias in &metadata.aliases {
            let _ = writeln!(out, "- `{alias}`");
        }
        let _ = writeln!(out);
    }

    if let Some(dep) = &metadata.deprecated {
        let _ = writeln!(out, "> **Deprecated:** {dep}");
        let _ = writeln!(out);
    }
}

fn write_variant_case(out: &mut String, graph: &SchemaGraph, case: &VariantCaseType) {
    match &case.payload {
        None => {
            let _ = writeln!(out, "- `{}`", case.name);
        }
        Some(p) => {
            let _ = writeln!(out, "- `{}`: `{}`", case.name, type_to_cli_text(graph, p));
        }
    }
    if let Some(doc) = &case.metadata.doc {
        let _ = writeln!(out, "  - {doc}");
    }
    write_inline_examples(out, &case.metadata);
}

fn write_union_branches(out: &mut String, graph: &SchemaGraph, spec: &UnionSpec) {
    let _ = writeln!(out, "### Branches");
    let _ = writeln!(out);
    for branch in &spec.branches {
        write_union_branch(out, graph, branch);
    }
    let _ = writeln!(out);
}

fn write_union_branch(out: &mut String, graph: &SchemaGraph, branch: &UnionBranch) {
    let _ = writeln!(
        out,
        "- `{}` ⟵ {}: `{}`",
        branch.tag,
        describe_discriminator(&branch.discriminator),
        type_to_cli_text(graph, &branch.body)
    );
    if let Some(doc) = &branch.metadata.doc {
        let _ = writeln!(out, "  - {doc}");
    }
    write_inline_examples(out, &branch.metadata);
}

fn describe_discriminator(rule: &DiscriminatorRule) -> String {
    match rule {
        DiscriminatorRule::Prefix { prefix } => format!("value starts with `{prefix}`"),
        DiscriminatorRule::Suffix { suffix } => format!("value ends with `{suffix}`"),
        DiscriminatorRule::Contains { substring } => format!("value contains `{substring}`"),
        DiscriminatorRule::Regex { regex } => format!("value matches regex `{regex}`"),
        DiscriminatorRule::FieldEquals(disc) => match &disc.literal {
            Some(lit) => format!("field `{}` == `{lit}`", disc.field_name),
            None => format!("field `{}` present", disc.field_name),
        },
        DiscriminatorRule::FieldAbsent { field_name } => format!("field `{field_name}` absent"),
    }
}

fn write_inline_examples(out: &mut String, metadata: &MetadataEnvelope) {
    if metadata.examples.is_empty() {
        return;
    }
    for example in &metadata.examples {
        let _ = writeln!(out, "  - example: `{example}`");
    }
}
