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

//! How a schema type is spelled in Go.
//!
//! The mapping is the same in both bridge modes, which is the point of putting
//! the value vocabulary in the shared `core/values` package: an agent and an
//! external client generated from the same schema get the *same* Go types, so
//! one domain package serves both sides. Only the call code differs by mode.
//!
//! Every distinction the schema draws has to survive, because the item codecs
//! are derived from the source schema rather than from the Go type. A schema
//! `text` is `values.Text` and not `string`; a `char` is `values.Char` and not
//! `rune`, which is only an alias for `int32`. Collapsing either would make two
//! different schemas produce the same Go type and the wrong wire encoding.

use crate::bridge_gen::go::go_writer::GoWriter;
use golem_common::schema::schema_type::SchemaType;

/// The import path of the shared value vocabulary.
pub const VALUES_PKG: &str = "github.com/golemcloud/golem/sdks/go/core/values";

/// The package qualifier the generated code binds `VALUES_PKG` to.
pub const VALUES: &str = "values";

/// Renders a Go type for a schema type that needs no named declaration.
///
/// `named` resolves a type that does have one — a record, variant, enum, flags
/// or union — to its generated name; the caller owns that table, so it is
/// passed in rather than reached for here.
///
/// Imports are recorded on `writer` as they are needed, since an unused import
/// is a compile error in Go and the set is not knowable before the walk.
pub fn render<'a>(
    typ: &'a SchemaType,
    named: &impl Fn(&SchemaType) -> Option<String>,
    resolve: &impl Fn(&'a SchemaType) -> &'a SchemaType,
    writer: &mut GoWriter,
) -> anyhow::Result<String> {
    if let Some(name) = named(typ) {
        return Ok(name);
    }

    let values = |writer: &mut GoWriter| {
        writer.import(VALUES_PKG);
        VALUES
    };

    let resolved = resolve(typ);
    Ok(match resolved {
        SchemaType::Bool { .. } => "bool".to_string(),
        SchemaType::S8 { .. } => "int8".to_string(),
        SchemaType::S16 { .. } => "int16".to_string(),
        SchemaType::S32 { .. } => "int32".to_string(),
        SchemaType::S64 { .. } => "int64".to_string(),
        SchemaType::U8 { .. } => "uint8".to_string(),
        SchemaType::U16 { .. } => "uint16".to_string(),
        SchemaType::U32 { .. } => "uint32".to_string(),
        SchemaType::U64 { .. } => "uint64".to_string(),
        SchemaType::F32 { .. } => "float32".to_string(),
        SchemaType::F64 { .. } => "float64".to_string(),
        SchemaType::String { .. } => "string".to_string(),

        // A named type, not `rune`: rune is an alias for int32, so collapsing
        // it would make char and s32 the same Go type.
        SchemaType::Char { .. } => format!("{}.Char", values(writer)),
        // Likewise text is not string, binary not []byte, path and url not
        // string — each marks an intent the schema draws and the codec reads.
        SchemaType::Text { .. } => format!("{}.Text", values(writer)),
        SchemaType::Binary { .. } => format!("{}.Binary", values(writer)),
        SchemaType::Path { .. } => format!("{}.Path", values(writer)),
        SchemaType::Url { .. } => format!("{}.URL", values(writer)),

        SchemaType::Datetime { .. } => {
            writer.import("time");
            "time.Time".to_string()
        }
        SchemaType::Duration { .. } => {
            writer.import("time");
            "time.Duration".to_string()
        }

        // An explicit Option rather than *T. Both lower to option<T>, but a
        // pointer is ambiguous when it nests — Option[Option[T]] is clear where
        // **T is not — and generated code has to be uniform.
        SchemaType::Option { inner, .. } => format!(
            "{}.Option[{}]",
            values(writer),
            render(inner, named, resolve, writer)?
        ),

        SchemaType::List { element, .. } => {
            format!("[]{}", render(element, named, resolve, writer)?)
        }
        // The length is part of the type, which is what keeps a fixed list
        // distinct from a list in the generated Go as well as in the schema.
        SchemaType::FixedList {
            element, length, ..
        } => {
            format!("[{length}]{}", render(element, named, resolve, writer)?)
        }

        // A schema map is an ordered list of pairs whose keys need not be
        // strings, so it is not a Go map: a Go map would lose the order and
        // could not hold a key type that is not comparable. The Rust bridge
        // spells the same thing Vec<(K, V)>.
        SchemaType::Map { key, value, .. } => format!(
            "[]{}.MapEntry[{}, {}]",
            values(writer),
            render(key, named, resolve, writer)?,
            render(value, named, resolve, writer)?
        ),

        SchemaType::Tuple { elements, .. } => {
            let mut rendered = Vec::with_capacity(elements.len());
            for element in elements {
                rendered.push(render(element, named, resolve, writer)?);
            }
            match rendered.len() {
                // A 1-tuple has no TupleN; it is its element, which is what the
                // wire carries anyway.
                1 => rendered.pop().expect("one element"),
                n @ 2..=8 => format!("{}.Tuple{n}[{}]", values(writer), rendered.join(", ")),
                n => anyhow::bail!(
                    "a {n}-element tuple has no Go spelling: the SDK provides Tuple2 through Tuple8"
                ),
            }
        }

        // A unit arm becomes struct{}, Go's zero-sized type, so the arm is
        // still addressable without carrying a value.
        SchemaType::Result { spec, .. } => {
            let ok = match spec.ok.as_deref() {
                Some(typ) => render(typ, named, resolve, writer)?,
                None => "struct{}".to_string(),
            };
            let err = match spec.err.as_deref() {
                Some(typ) => render(typ, named, resolve, writer)?,
                None => "struct{}".to_string(),
            };
            format!("{}.Result[{ok}, {err}]", values(writer))
        }

        SchemaType::Record { .. }
        | SchemaType::Variant { .. }
        | SchemaType::Enum { .. }
        | SchemaType::Flags { .. }
        | SchemaType::Union { .. } => anyhow::bail!(
            "a composite schema type reached the Go type mapping without a generated name: \
             {resolved:?}"
        ),
        SchemaType::Ref { .. } => {
            unreachable!("a ref is resolved to its body before it reaches here")
        }

        SchemaType::Quantity { .. }
        | SchemaType::Secret { .. }
        | SchemaType::QuotaToken { .. }
        | SchemaType::PermissionCard { .. }
        | SchemaType::Future { .. }
        | SchemaType::Stream { .. } => {
            anyhow::bail!("the Go bridge does not yet spell this schema type: {resolved:?}")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::MetadataEnvelope;
    use golem_common::schema::schema_type::ResultSpec;
    use test_r::test;

    fn unnamed(_: &SchemaType) -> Option<String> {
        None
    }

    fn identity(typ: &SchemaType) -> &SchemaType {
        typ
    }

    fn meta() -> MetadataEnvelope {
        MetadataEnvelope::default()
    }

    fn go_type(typ: &SchemaType) -> String {
        let mut writer = GoWriter::new();
        render(typ, &unnamed, &identity, &mut writer).expect("a Go type")
    }

    fn go_type_with_imports(typ: &SchemaType) -> (String, String) {
        let mut writer = GoWriter::new();
        let rendered = render(typ, &unnamed, &identity, &mut writer).expect("a Go type");
        writer.line("var _ = 1");
        (rendered, writer.finish("client"))
    }

    #[test]
    fn primitives_map_to_their_exact_go_width() {
        assert_eq!(go_type(&SchemaType::Bool { metadata: meta() }), "bool");
        assert_eq!(
            go_type(&SchemaType::S8 {
                restrictions: None,
                metadata: meta()
            }),
            "int8"
        );
        assert_eq!(
            go_type(&SchemaType::U64 {
                restrictions: None,
                metadata: meta()
            }),
            "uint64"
        );
        assert_eq!(
            go_type(&SchemaType::F32 {
                restrictions: None,
                metadata: meta()
            }),
            "float32"
        );
        assert_eq!(go_type(&SchemaType::String { metadata: meta() }), "string");
    }

    /// The codecs are derived from the source schema, so a distinction the
    /// schema draws must not collapse in Go. `rune` is an alias for `int32`, so
    /// a char spelled as `rune` would be the same type as an s32.
    #[test]
    fn semantic_types_stay_distinct_from_their_underlying_go_types() {
        assert_eq!(
            go_type(&SchemaType::Char { metadata: meta() }),
            "values.Char"
        );
        assert_ne!(
            go_type(&SchemaType::Char { metadata: meta() }),
            go_type(&SchemaType::S32 {
                restrictions: None,
                metadata: meta()
            })
        );
        assert_eq!(
            go_type(&SchemaType::Text {
                restrictions: Default::default(),
                metadata: meta()
            }),
            "values.Text"
        );
        assert_ne!(
            go_type(&SchemaType::Text {
                restrictions: Default::default(),
                metadata: meta()
            }),
            go_type(&SchemaType::String { metadata: meta() })
        );
        assert_eq!(
            go_type(&SchemaType::Binary {
                restrictions: Default::default(),
                metadata: meta()
            }),
            "values.Binary"
        );
    }

    /// A list and a fixed list are different schema types and must stay
    /// different Go types, or a round trip through the generated client would
    /// silently change the encoding.
    #[test]
    fn a_fixed_list_keeps_its_length() {
        let element = Box::new(SchemaType::String { metadata: meta() });
        assert_eq!(
            go_type(&SchemaType::List {
                element: element.clone(),
                metadata: meta()
            }),
            "[]string"
        );
        assert_eq!(
            go_type(&SchemaType::FixedList {
                element,
                length: 4,
                metadata: meta()
            }),
            "[4]string"
        );
    }

    /// A schema map is an ordered list of pairs with unrestricted keys, so it
    /// does not become a Go map.
    #[test]
    fn a_map_is_a_slice_of_entries() {
        assert_eq!(
            go_type(&SchemaType::Map {
                key: Box::new(SchemaType::String { metadata: meta() }),
                value: Box::new(SchemaType::Bool { metadata: meta() }),
                metadata: meta()
            }),
            "[]values.MapEntry[string, bool]"
        );
    }

    #[test]
    fn options_nest_without_ambiguity() {
        let inner = SchemaType::Option {
            inner: Box::new(SchemaType::String { metadata: meta() }),
            metadata: meta(),
        };
        assert_eq!(
            go_type(&SchemaType::Option {
                inner: Box::new(inner),
                metadata: meta()
            }),
            "values.Option[values.Option[string]]"
        );
    }

    #[test]
    fn tuples_use_the_arity_specific_type() {
        let elements = vec![
            SchemaType::String { metadata: meta() },
            SchemaType::Bool { metadata: meta() },
        ];
        assert_eq!(
            go_type(&SchemaType::Tuple {
                elements,
                metadata: meta()
            }),
            "values.Tuple2[string, bool]"
        );
        // A 1-tuple is its element: there is no Tuple1, and the wire carries
        // exactly the element.
        assert_eq!(
            go_type(&SchemaType::Tuple {
                elements: vec![SchemaType::Bool { metadata: meta() }],
                metadata: meta()
            }),
            "bool"
        );
    }

    #[test]
    fn a_tuple_beyond_the_provided_arities_is_refused_with_a_reason() {
        let elements = (0..9)
            .map(|_| SchemaType::Bool { metadata: meta() })
            .collect();
        let mut writer = GoWriter::new();
        let err = render(
            &SchemaType::Tuple {
                elements,
                metadata: meta(),
            },
            &unnamed,
            &identity,
            &mut writer,
        )
        .expect_err("a 9-tuple has no Go spelling");
        assert!(
            err.to_string().contains("Tuple2 through Tuple8"),
            "the error should say what is available: {err}"
        );
    }

    #[test]
    fn a_unit_result_arm_becomes_a_zero_sized_struct() {
        assert_eq!(
            go_type(&SchemaType::Result {
                spec: ResultSpec {
                    ok: None,
                    err: Some(Box::new(SchemaType::String { metadata: meta() })),
                },
                metadata: meta()
            }),
            "values.Result[struct{}, string]"
        );
    }

    /// An unused import is a compile error in Go, so the writer must record an
    /// import exactly when the rendered type needs one.
    #[test]
    fn rendering_records_the_imports_the_type_needs() {
        let (rendered, file) = go_type_with_imports(&SchemaType::Datetime { metadata: meta() });
        assert_eq!(rendered, "time.Time");
        assert!(file.contains("\"time\""), "{file}");
        assert!(!file.contains("core/values"), "{file}");

        let (_, file) = go_type_with_imports(&SchemaType::Text {
            restrictions: Default::default(),
            metadata: meta(),
        });
        assert!(file.contains("core/values"), "{file}");
        assert!(!file.contains("\"time\""), "{file}");

        let (_, file) = go_type_with_imports(&SchemaType::Bool { metadata: meta() });
        assert!(!file.contains("import"), "a bool needs no import: {file}");
    }

    /// A composite reaching here means the caller's name table was incomplete;
    /// emitting a wrong type would be far worse than failing.
    #[test]
    fn a_composite_without_a_name_is_an_error() {
        let mut writer = GoWriter::new();
        let err = render(
            &SchemaType::Record {
                fields: vec![],
                metadata: meta(),
            },
            &unnamed,
            &identity,
            &mut writer,
        )
        .expect_err("a record needs a generated name");
        assert!(
            err.to_string().contains("without a generated name"),
            "{err}"
        );
    }
}
