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

//! The named Go declarations: the five schema types Go cannot spell inline.
//!
//! A record is a struct and flags are a struct of booleans, which are the
//! obvious shapes. The other three need a decision:
//!
//! - An **enum** becomes a named string type with one constant per case. The
//!   wire carries a case index, so the codec maps between the two; the constant
//!   holds the schema's own case name, which is what makes a log line and a
//!   debugger readable. An integer-backed enum would make the codec marginally
//!   simpler and every other reader's job worse.
//! - A **variant** becomes a sealed interface with one struct per case. Go has
//!   no sum type, and the alternative — one struct with a tag and a pointer per
//!   case — lets a caller build a value that names one case and carries
//!   another. With an interface, a case *is* its payload and cannot be
//!   half-constructed.
//! - A **union** becomes the same shape. It is also a closed sum; only how the
//!   wire recognises a branch differs, and that is the codec's business.
//!
//! Go switches are not exhaustive, so a caller matching on a variant gets no
//! compile-time warning when a case is added. The sealed interface at least
//! stops them constructing one that does not exist.
//!
//! Names are transliterated mechanically, so a schema `order-id` becomes
//! `OrderId` and not Go's conventional `OrderID`. Applying Go's initialism list
//! would read better in isolation but makes the mapping back to the schema name
//! depend on knowing that list, and it is a rule none of the other five
//! generators apply. Generated files carry Go's DO NOT EDIT marker, so the
//! linter check that would object to `OrderId` skips them anyway.

use crate::bridge_gen::go::go::{to_exported_ident, to_field_ident, unique_idents};
use crate::bridge_gen::go::go_writer::GoWriter;
use golem_common::schema::MetadataEnvelope;
use golem_common::schema::schema_type::{NamedFieldType, SchemaType, VariantCaseType};

/// Renders the Go type of a schema type that needs no declaration of its own.
/// The generator owns the walk, so it is passed in.
pub type RenderType<'a> = dyn Fn(&SchemaType, &mut GoWriter) -> anyhow::Result<String> + 'a;

/// Writes the declaration for a named schema type. Returns false when the type
/// needs no declaration, which the caller treats as "already spelled inline".
pub fn write(
    name: &str,
    typ: &SchemaType,
    render: &RenderType<'_>,
    writer: &mut GoWriter,
) -> anyhow::Result<bool> {
    match typ {
        SchemaType::Record {
            fields, metadata, ..
        } => {
            write_record(name, fields, metadata, render, writer)?;
            Ok(true)
        }
        SchemaType::Enum {
            cases, metadata, ..
        } => {
            write_enum(name, cases, metadata, writer);
            Ok(true)
        }
        SchemaType::Flags {
            flags, metadata, ..
        } => {
            write_flags(name, flags, metadata, writer);
            Ok(true)
        }
        SchemaType::Variant {
            cases, metadata, ..
        } => {
            write_variant(name, cases, metadata, render, writer)?;
            Ok(true)
        }
        SchemaType::Union { spec, metadata, .. } => {
            let cases = spec
                .branches
                .iter()
                .map(|branch| VariantCaseType {
                    name: branch.tag.clone(),
                    payload: Some(branch.body.clone()),
                    metadata: branch.metadata.clone(),
                })
                .collect::<Vec<_>>();
            write_variant(name, &cases, metadata, render, writer)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn write_record(
    name: &str,
    fields: &[NamedFieldType],
    metadata: &MetadataEnvelope,
    render: &RenderType<'_>,
    writer: &mut GoWriter,
) -> anyhow::Result<()> {
    // Rendering first records the imports the field types need, before the
    // declaration is written.
    let mut rendered = Vec::with_capacity(fields.len());
    for field in fields {
        rendered.push(render(&field.body, writer)?);
    }
    let idents = unique_idents(fields.iter().map(|f| to_field_ident(&f.name)).collect());

    write_doc(name, metadata, writer);
    if fields.is_empty() {
        writer.line(format!("type {name} struct{{}}"));
        writer.blank();
        return Ok(());
    }
    writer.line(format!("type {name} struct {{"));
    writer.indent();
    for (idx, field) in fields.iter().enumerate() {
        write_member_doc(&idents[idx], &field.metadata, writer);
        writer.line(format!("{} {}", idents[idx], rendered[idx]));
    }
    writer.dedent();
    writer.line("}");
    writer.blank();
    Ok(())
}

fn write_enum(name: &str, cases: &[String], metadata: &MetadataEnvelope, writer: &mut GoWriter) {
    write_doc(name, metadata, writer);
    writer.line(format!("type {name} string"));
    writer.blank();

    // The constant is named <Type><Case> so two enums in the same package can
    // both have a "pending" case.
    let idents = unique_idents(
        cases
            .iter()
            .map(|case| format!("{name}{}", to_exported_ident(case)))
            .collect(),
    );
    writer.line("const (");
    writer.indent();
    for (idx, case) in cases.iter().enumerate() {
        writer.line(format!("{} {name} = {}", idents[idx], go_string(case)));
    }
    writer.dedent();
    writer.line(")");
    writer.blank();

    // The declared cases, in schema order. The codec needs the index, and a
    // caller occasionally wants to enumerate them.
    writer.line(format!(
        "// All{name} lists the declared cases, in schema order."
    ));
    writer.line(format!("var All{name} = []{name}{{"));
    writer.indent();
    for ident in &idents {
        writer.line(format!("{ident},"));
    }
    writer.dedent();
    writer.line("}");
    writer.blank();
}

fn write_flags(name: &str, flags: &[String], metadata: &MetadataEnvelope, writer: &mut GoWriter) {
    write_doc(name, metadata, writer);
    if flags.is_empty() {
        writer.line(format!("type {name} struct{{}}"));
        writer.blank();
        return;
    }
    let idents = unique_idents(flags.iter().map(|flag| to_field_ident(flag)).collect());
    writer.line(format!("type {name} struct {{"));
    writer.indent();
    for ident in &idents {
        writer.line(format!("{ident} bool"));
    }
    writer.dedent();
    writer.line("}");
    writer.blank();
}

fn write_variant(
    name: &str,
    cases: &[VariantCaseType],
    metadata: &MetadataEnvelope,
    render: &RenderType<'_>,
    writer: &mut GoWriter,
) -> anyhow::Result<()> {
    let mut payloads = Vec::with_capacity(cases.len());
    for case in cases {
        payloads.push(match &case.payload {
            Some(payload) => Some(render(payload, writer)?),
            None => None,
        });
    }
    let idents = unique_idents(
        cases
            .iter()
            .map(|case| format!("{name}{}", to_exported_ident(&case.name)))
            .collect(),
    );
    // The sealing method is named after the type, so two variants in the same
    // package do not seal each other.
    let seal = format!("is{name}");

    write_doc(name, metadata, writer);
    writer.line(format!(
        "// A value is one of the {name}… types below; nothing else can implement it."
    ));
    writer.line(format!("type {name} interface {{ {seal}() }}"));
    writer.blank();

    for (idx, case) in cases.iter().enumerate() {
        let ident = &idents[idx];
        write_member_doc(ident, &case.metadata, writer);
        match &payloads[idx] {
            // A case with a payload carries it as Value, so a caller writes
            // ShapeCircle{Value: r} whatever the payload's shape is.
            Some(payload) => writer.line(format!("type {ident} struct {{ Value {payload} }}")),
            None => writer.line(format!("type {ident} struct{{}}")),
        }
        writer.line(format!("func ({ident}) {seal}() {{}}"));
        writer.blank();
    }
    Ok(())
}

fn write_doc(name: &str, metadata: &MetadataEnvelope, writer: &mut GoWriter) {
    match metadata.doc.as_deref() {
        // A Go doc comment starts with the identifier it documents, which is
        // what the linter's ST1021 checks.
        Some(doc) if !doc.trim().is_empty() => {
            writer.doc(&format!("{name} is {}", lower_first(doc)))
        }
        _ => writer.doc(&format!("{name} is a generated type.")),
    }
}

fn write_member_doc(name: &str, metadata: &MetadataEnvelope, writer: &mut GoWriter) {
    if let Some(doc) = metadata.doc.as_deref()
        && !doc.trim().is_empty()
    {
        writer.doc(&format!("{name} is {}", lower_first(doc)));
    }
}

/// Lower-cases the first character so the schema's sentence reads as the tail
/// of "<Name> is …". A doc that already starts lower-case, or with something
/// that has no case, is left alone.
fn lower_first(doc: &str) -> String {
    let doc = doc.trim();
    let mut chars = doc.chars();
    match chars.next() {
        Some(first) if first.is_uppercase() => {
            // Only when the second character is not also upper-case, so an
            // acronym such as "HTTP status" is not mangled.
            if doc.chars().nth(1).is_some_and(|c| c.is_uppercase()) {
                doc.to_string()
            } else {
                first.to_lowercase().collect::<String>() + chars.as_str()
            }
        }
        _ => doc.to_string(),
    }
}

/// A Go string literal. Schema case names are identifiers, but a quote or a
/// backslash would still produce source that does not parse.
fn go_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_gen::go::type_ref;
    use golem_common::schema::schema_type::{DiscriminatorRule, FieldDiscriminator};
    use golem_common::schema::schema_type::{UnionBranch, UnionSpec};
    use test_r::test;

    fn meta() -> MetadataEnvelope {
        MetadataEnvelope::default()
    }

    fn documented(doc: &str) -> MetadataEnvelope {
        MetadataEnvelope {
            doc: Some(doc.to_string()),
            ..MetadataEnvelope::default()
        }
    }

    fn render_inline(typ: &SchemaType, writer: &mut GoWriter) -> anyhow::Result<String> {
        type_ref::render(typ, &|_| None, &|t| t, writer)
    }

    fn emit(name: &str, typ: &SchemaType) -> String {
        let mut writer = GoWriter::new();
        let declared = write(name, typ, &render_inline, &mut writer).expect("a declaration");
        assert!(declared, "{name} should need a declaration");
        writer.finish("client")
    }

    fn field(name: &str, body: SchemaType) -> NamedFieldType {
        NamedFieldType {
            name: name.to_string(),
            body,
            metadata: meta(),
        }
    }

    #[test]
    fn a_record_becomes_a_struct_with_exported_fields() {
        let typ = SchemaType::Record {
            fields: vec![
                field("order-id", SchemaType::String { metadata: meta() }),
                field(
                    "item-count",
                    SchemaType::U32 {
                        restrictions: None,
                        metadata: meta(),
                    },
                ),
            ],
            metadata: documented("an order placed by a customer."),
        };
        let rendered = emit("Order", &typ);
        assert!(
            rendered.contains("// Order is an order placed by a customer."),
            "{rendered}"
        );
        assert!(
            rendered.contains("type Order struct {\n\tOrderId string\n\tItemCount uint32\n}"),
            "{rendered}"
        );
    }

    #[test]
    fn an_empty_record_is_still_a_struct() {
        let typ = SchemaType::Record {
            fields: vec![],
            metadata: meta(),
        };
        assert!(emit("Empty", &typ).contains("type Empty struct{}"));
    }

    /// Go drops the separator, so field names that stay distinct in the schema
    /// collide here and have to be disambiguated.
    #[test]
    fn colliding_field_names_are_disambiguated() {
        let typ = SchemaType::Record {
            fields: vec![
                field("first-name", SchemaType::String { metadata: meta() }),
                field("first_name", SchemaType::String { metadata: meta() }),
            ],
            metadata: meta(),
        };
        let rendered = emit("Person", &typ);
        assert!(rendered.contains("FirstName string"), "{rendered}");
        assert!(rendered.contains("FirstName2 string"), "{rendered}");
    }

    /// The constant carries the schema's own case name, so a log line and a
    /// debugger both read it, and the type prefix keeps two enums apart.
    #[test]
    fn an_enum_becomes_a_string_type_with_prefixed_constants() {
        let typ = SchemaType::Enum {
            cases: vec!["pending".into(), "in-transit".into()],
            metadata: meta(),
        };
        let rendered = emit("Status", &typ);
        assert!(rendered.contains("type Status string"), "{rendered}");
        assert!(
            rendered.contains("StatusPending Status = \"pending\""),
            "{rendered}"
        );
        assert!(
            rendered.contains("StatusInTransit Status = \"in-transit\""),
            "{rendered}"
        );
        assert!(rendered.contains("var AllStatus = []Status{"), "{rendered}");
    }

    #[test]
    fn flags_become_a_struct_of_booleans() {
        let typ = SchemaType::Flags {
            flags: vec!["read".into(), "write".into()],
            metadata: meta(),
        };
        let rendered = emit("Permissions", &typ);
        assert!(
            rendered.contains("type Permissions struct {\n\tRead bool\n\tWrite bool\n}"),
            "{rendered}"
        );
    }

    /// A sealed interface means a caller cannot build a value that names one
    /// case and carries another, which a tag-plus-pointers struct would allow.
    #[test]
    fn a_variant_becomes_a_sealed_interface_with_one_struct_per_case() {
        let typ = SchemaType::Variant {
            cases: vec![
                VariantCaseType {
                    name: "circle".into(),
                    payload: Some(SchemaType::F64 {
                        restrictions: None,
                        metadata: meta(),
                    }),
                    metadata: meta(),
                },
                VariantCaseType {
                    name: "unknown".into(),
                    payload: None,
                    metadata: meta(),
                },
            ],
            metadata: meta(),
        };
        let rendered = emit("Shape", &typ);
        assert!(
            rendered.contains("type Shape interface { isShape() }"),
            "{rendered}"
        );
        assert!(
            rendered.contains("type ShapeCircle struct { Value float64 }"),
            "{rendered}"
        );
        assert!(
            rendered.contains("func (ShapeCircle) isShape() {}"),
            "{rendered}"
        );
        assert!(
            rendered.contains("type ShapeUnknown struct{}"),
            "a payloadless case is an empty struct: {rendered}"
        );
    }

    /// Two variants in one package must not seal each other, or a case of one
    /// would satisfy the other's interface.
    #[test]
    fn the_sealing_method_is_named_after_its_own_type() {
        let case = |name: &str| VariantCaseType {
            name: name.to_string(),
            payload: None,
            metadata: meta(),
        };
        let first = emit(
            "Shape",
            &SchemaType::Variant {
                cases: vec![case("a")],
                metadata: meta(),
            },
        );
        let second = emit(
            "Colour",
            &SchemaType::Variant {
                cases: vec![case("a")],
                metadata: meta(),
            },
        );
        assert!(first.contains("isShape()"), "{first}");
        assert!(second.contains("isColour()"), "{second}");
        assert!(!second.contains("isShape"), "{second}");
    }

    /// A union is also a closed sum; only how the wire recognises a branch
    /// differs, so it gets the same Go shape as a variant.
    #[test]
    fn a_union_gets_the_same_shape_as_a_variant() {
        let typ = SchemaType::Union {
            spec: UnionSpec {
                branches: vec![UnionBranch {
                    tag: "inline".into(),
                    body: SchemaType::String { metadata: meta() },
                    discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                        field_name: "kind".into(),
                        literal: Some("inline".into()),
                    }),
                    metadata: meta(),
                }],
            },
            metadata: meta(),
        };
        let rendered = emit("Payload", &typ);
        assert!(
            rendered.contains("type Payload interface { isPayload() }"),
            "{rendered}"
        );
        assert!(
            rendered.contains("type PayloadInline struct { Value string }"),
            "{rendered}"
        );
    }

    /// A type Go can spell inline has no declaration, and saying so is how the
    /// caller knows not to emit one.
    #[test]
    fn an_inline_type_needs_no_declaration() {
        let mut writer = GoWriter::new();
        let declared = write(
            "Ignored",
            &SchemaType::String { metadata: meta() },
            &render_inline,
            &mut writer,
        )
        .expect("no error");
        assert!(!declared);
        assert_eq!(
            writer.finish("client"),
            "// Code generated by golem-cli. DO NOT EDIT.\n\npackage client\n"
        );
    }

    /// The imports a field type needs are recorded while the declaration is
    /// built, since an unused — or missing — import is a compile error in Go.
    #[test]
    fn a_declaration_records_the_imports_its_fields_need() {
        let typ = SchemaType::Record {
            fields: vec![field(
                "placed-at",
                SchemaType::Datetime { metadata: meta() },
            )],
            metadata: meta(),
        };
        let rendered = emit("Order", &typ);
        assert!(rendered.contains("import \"time\""), "{rendered}");
        assert!(rendered.contains("PlacedAt time.Time"), "{rendered}");
    }

    /// A Go doc comment has to start with the identifier it documents, so the
    /// schema's sentence becomes the tail of "<Name> is …".
    #[test]
    fn docs_are_rewritten_to_start_with_the_identifier() {
        assert_eq!(lower_first("An order."), "an order.");
        // An acronym is left alone rather than lower-cased into nonsense.
        assert_eq!(lower_first("HTTP status."), "HTTP status.");
        assert_eq!(lower_first("already lower."), "already lower.");
    }

    #[test]
    fn a_case_name_with_a_quote_still_produces_parsable_source() {
        let typ = SchemaType::Enum {
            cases: vec!["say \"hi\"".into()],
            metadata: meta(),
        };
        assert!(emit("Greeting", &typ).contains("\"say \\\"hi\\\"\""));
    }
}
