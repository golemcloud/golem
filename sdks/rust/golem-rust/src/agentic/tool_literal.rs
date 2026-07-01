// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Interpretation of metadata-time literals (`#[arg(default = ...)]` and
//! `value_is(...)`) against their target type node, producing the
//! `SchemaValue` the tool model stores for option/positional defaults and
//! `value-is` constraint references.

use crate::agentic::extended_tool_type::ToolBuildError;
use crate::schema::schema_value::TextValuePayload;
use crate::schema::{SchemaGraph, SchemaType, SchemaValue};

/// A literal written in a tool authoring attribute, captured before it is
/// interpreted against the referenced type node. The macro builds one of these
/// from the Rust attribute expression; the value type itself determines how it
/// is interpreted (e.g. a string against an `enum` type selects a case).
#[derive(Clone, Debug, PartialEq)]
pub enum ToolLiteral {
    Bool(bool),
    /// Integer literal, widened to `i128` so it can carry any signed or
    /// unsigned target down to the concrete numeric type.
    Int(i128),
    Float(f64),
    Char(char),
    Str(String),
    List(Vec<ToolLiteral>),
    Map(Vec<(ToolLiteral, ToolLiteral)>),
}

/// Interprets `lit` against the root type of `graph` (resolving any leading
/// `Ref` indirections), returning the `SchemaValue` to store as a default or
/// `value-is` literal.
pub fn literal_to_schema_value(
    graph: &SchemaGraph,
    lit: &ToolLiteral,
) -> Result<SchemaValue, ToolBuildError> {
    let root = graph.root.clone();
    interpret(graph, &root, lit)
}

/// Interprets `lit` as a `value-is` comparand literal against `graph`, honoring
/// the WIT "any occurrence / element equals this literal" rule: when the literal
/// is not a whole-value match and the comparand resolves (through `Option`
/// wrappers) to a list, the literal is interpreted as a single element value; to
/// a map, as a single map-value.
///
/// `graph` is the comparand graph the runtime registered for the referenced
/// argument (`value_is_comparand_graph` for an option, the declared type for a
/// fixed positional, `list<item>` for a tail). Because this peels exactly one
/// element/value level from the *whole declared type*, `value-is("xs", item)` is
/// accepted as an item literal whether `xs` is a `Vec<T>`, a `type Alias =
/// Vec<T>`, a map, or an ancestor-supplied global, and stays consistent with the
/// `value_is_compatible` check applied next.
pub fn value_is_literal_to_schema_value(
    graph: &SchemaGraph,
    lit: &ToolLiteral,
) -> Result<SchemaValue, ToolBuildError> {
    let direct = match literal_to_schema_value(graph, lit) {
        Ok(value) => return Ok(value),
        Err(direct) => direct,
    };
    // The literal is not a whole-value match; for a list-shaped comparand,
    // interpret it as one element.
    let mut ty = match graph.resolve_ref(&graph.root) {
        Ok(ty) => ty,
        Err(_) => return Err(direct),
    };
    while let SchemaType::Option { inner, .. } = ty {
        match graph.resolve_ref(inner) {
            Ok(next) => ty = next,
            Err(_) => return Err(direct),
        }
    }
    match ty {
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            let element = (**element).clone();
            interpret(graph, &element, lit).map_err(|_| direct)
        }
        SchemaType::Map { value, .. } => {
            let map_value = (**value).clone();
            interpret(graph, &map_value, lit).map_err(|_| direct)
        }
        _ => Err(direct),
    }
}

fn mismatch(ty: &SchemaType, lit: &ToolLiteral) -> ToolBuildError {
    ToolBuildError::DefaultTypeMismatch(format!("literal {lit:?} is not valid for type {ty:?}"))
}

fn interpret(
    graph: &SchemaGraph,
    ty: &SchemaType,
    lit: &ToolLiteral,
) -> Result<SchemaValue, ToolBuildError> {
    // Resolve through any number of `Ref` indirections first.
    let resolved = graph
        .resolve_ref(ty)
        .map_err(|e| ToolBuildError::DefaultTypeMismatch(e.to_string()))?;

    match resolved {
        SchemaType::Bool { .. } => match lit {
            ToolLiteral::Bool(b) => Ok(SchemaValue::Bool(*b)),
            _ => Err(mismatch(resolved, lit)),
        },
        SchemaType::S8 { .. } => int_value(resolved, lit, i8::MIN as i128, i8::MAX as i128, |v| {
            SchemaValue::S8(v as i8)
        }),
        SchemaType::S16 { .. } => {
            int_value(resolved, lit, i16::MIN as i128, i16::MAX as i128, |v| {
                SchemaValue::S16(v as i16)
            })
        }
        SchemaType::S32 { .. } => {
            int_value(resolved, lit, i32::MIN as i128, i32::MAX as i128, |v| {
                SchemaValue::S32(v as i32)
            })
        }
        SchemaType::S64 { .. } => {
            int_value(resolved, lit, i64::MIN as i128, i64::MAX as i128, |v| {
                SchemaValue::S64(v as i64)
            })
        }
        SchemaType::U8 { .. } => int_value(resolved, lit, 0, u8::MAX as i128, |v| {
            SchemaValue::U8(v as u8)
        }),
        SchemaType::U16 { .. } => int_value(resolved, lit, 0, u16::MAX as i128, |v| {
            SchemaValue::U16(v as u16)
        }),
        SchemaType::U32 { .. } => int_value(resolved, lit, 0, u32::MAX as i128, |v| {
            SchemaValue::U32(v as u32)
        }),
        SchemaType::U64 { .. } => int_value(resolved, lit, 0, u64::MAX as i128, |v| {
            SchemaValue::U64(v as u64)
        }),
        SchemaType::F32 { .. } => match lit {
            ToolLiteral::Float(f) => Ok(SchemaValue::F32(*f as f32)),
            ToolLiteral::Int(i) => Ok(SchemaValue::F32(*i as f32)),
            _ => Err(mismatch(resolved, lit)),
        },
        SchemaType::F64 { .. } => match lit {
            ToolLiteral::Float(f) => Ok(SchemaValue::F64(*f)),
            ToolLiteral::Int(i) => Ok(SchemaValue::F64(*i as f64)),
            _ => Err(mismatch(resolved, lit)),
        },
        SchemaType::Char { .. } => match lit {
            ToolLiteral::Char(c) => Ok(SchemaValue::Char(*c)),
            _ => Err(mismatch(resolved, lit)),
        },
        SchemaType::String { .. } => match lit {
            ToolLiteral::Str(s) => Ok(SchemaValue::String(s.clone())),
            _ => Err(mismatch(resolved, lit)),
        },
        SchemaType::Text { .. } => match lit {
            ToolLiteral::Str(s) => Ok(SchemaValue::Text(TextValuePayload {
                text: s.clone(),
                language: None,
            })),
            _ => Err(mismatch(resolved, lit)),
        },
        SchemaType::Path { .. } => match lit {
            ToolLiteral::Str(s) => Ok(SchemaValue::Path { path: s.clone() }),
            _ => Err(mismatch(resolved, lit)),
        },
        SchemaType::Url { .. } => match lit {
            ToolLiteral::Str(s) => Ok(SchemaValue::Url { url: s.clone() }),
            _ => Err(mismatch(resolved, lit)),
        },
        SchemaType::Enum { cases, .. } => match lit {
            ToolLiteral::Str(s) => {
                let case = cases.iter().position(|c| c == s).ok_or_else(|| {
                    ToolBuildError::DefaultTypeMismatch(format!(
                        "enum case {s:?} is not one of {cases:?}"
                    ))
                })?;
                Ok(SchemaValue::Enum { case: case as u32 })
            }
            _ => Err(mismatch(resolved, lit)),
        },
        SchemaType::Option { inner, .. } => {
            let inner_value = interpret(graph, inner, lit)?;
            Ok(SchemaValue::Option {
                inner: Some(Box::new(inner_value)),
            })
        }
        SchemaType::List { element, .. } => match lit {
            ToolLiteral::List(items) => {
                let elements = items
                    .iter()
                    .map(|item| interpret(graph, element, item))
                    .collect::<Result<_, _>>()?;
                Ok(SchemaValue::List { elements })
            }
            _ => Err(mismatch(resolved, lit)),
        },
        SchemaType::FixedList {
            element, length, ..
        } => match lit {
            ToolLiteral::List(items) if items.len() == *length as usize => {
                let elements = items
                    .iter()
                    .map(|item| interpret(graph, element, item))
                    .collect::<Result<_, _>>()?;
                Ok(SchemaValue::FixedList { elements })
            }
            _ => Err(mismatch(resolved, lit)),
        },
        SchemaType::Map { key, value, .. } => match lit {
            ToolLiteral::Map(entries) => {
                let entries = entries
                    .iter()
                    .map(|(k, v)| Ok((interpret(graph, key, k)?, interpret(graph, value, v)?)))
                    .collect::<Result<_, _>>()?;
                Ok(SchemaValue::Map { entries })
            }
            // An empty array literal `[]` is the natural way to author an empty
            // map default; it parses as a (List) literal but carries no entries,
            // so it interprets as an empty map.
            ToolLiteral::List(items) if items.is_empty() => {
                Ok(SchemaValue::Map { entries: vec![] })
            }
            _ => Err(mismatch(resolved, lit)),
        },
        _ => Err(ToolBuildError::DefaultTypeMismatch(format!(
            "literals are not supported for type {resolved:?}"
        ))),
    }
}

fn int_value(
    ty: &SchemaType,
    lit: &ToolLiteral,
    min: i128,
    max: i128,
    build: impl FnOnce(i128) -> SchemaValue,
) -> Result<SchemaValue, ToolBuildError> {
    match lit {
        ToolLiteral::Int(i) => {
            if *i < min || *i > max {
                return Err(ToolBuildError::DefaultTypeMismatch(format!(
                    "integer literal {i} is out of range for {ty:?}"
                )));
            }
            Ok(build(*i))
        }
        _ => Err(mismatch(ty, lit)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SchemaType;
    use test_r::test;

    fn graph(root: SchemaType) -> SchemaGraph {
        SchemaGraph::anonymous(root)
    }

    #[test]
    fn string_literal() {
        let v =
            literal_to_schema_value(&graph(SchemaType::string()), &ToolLiteral::Str("hi".into()))
                .unwrap();
        assert_eq!(v, SchemaValue::String("hi".into()));
    }

    #[test]
    fn enum_case_by_name() {
        let enum_ty = SchemaType::Enum {
            cases: vec!["always".into(), "never".into(), "auto".into()],
            metadata: Default::default(),
        };
        let v = literal_to_schema_value(&graph(enum_ty), &ToolLiteral::Str("auto".into())).unwrap();
        assert_eq!(v, SchemaValue::Enum { case: 2 });
    }

    #[test]
    fn unknown_enum_case_errors() {
        let enum_ty = SchemaType::Enum {
            cases: vec!["always".into()],
            metadata: Default::default(),
        };
        let err =
            literal_to_schema_value(&graph(enum_ty), &ToolLiteral::Str("nope".into())).unwrap_err();
        assert!(matches!(err, ToolBuildError::DefaultTypeMismatch(_)));
    }

    #[test]
    fn u64_max_in_range() {
        let v = literal_to_schema_value(
            &graph(SchemaType::u64()),
            &ToolLiteral::Int(u64::MAX as i128),
        )
        .unwrap();
        assert_eq!(v, SchemaValue::U64(u64::MAX));
    }

    #[test]
    fn integer_out_of_range_errors() {
        let err =
            literal_to_schema_value(&graph(SchemaType::u32()), &ToolLiteral::Int(-1)).unwrap_err();
        assert!(matches!(err, ToolBuildError::DefaultTypeMismatch(_)));
    }

    #[test]
    fn path_literal() {
        let v = literal_to_schema_value(
            &graph(SchemaType::Path {
                spec: crate::schema::PathSpec {
                    direction: crate::schema::PathDirection::InOut,
                    kind: crate::schema::PathKind::Any,
                    allowed_mime_types: None,
                    allowed_extensions: None,
                },
                metadata: Default::default(),
            }),
            &ToolLiteral::Str(".git".into()),
        )
        .unwrap();
        assert_eq!(
            v,
            SchemaValue::Path {
                path: ".git".into()
            }
        );
    }

    #[test]
    fn list_of_strings() {
        let v = literal_to_schema_value(
            &graph(SchemaType::list(SchemaType::string())),
            &ToolLiteral::List(vec![
                ToolLiteral::Str("a".into()),
                ToolLiteral::Str("b".into()),
            ]),
        )
        .unwrap();
        assert_eq!(
            v,
            SchemaValue::List {
                elements: vec![
                    SchemaValue::String("a".into()),
                    SchemaValue::String("b".into())
                ]
            }
        );
    }

    #[test]
    fn map_of_strings() {
        let v = literal_to_schema_value(
            &graph(SchemaType::map(SchemaType::string(), SchemaType::string())),
            &ToolLiteral::Map(vec![(
                ToolLiteral::Str("k".into()),
                ToolLiteral::Str("v".into()),
            )]),
        )
        .unwrap();
        assert_eq!(
            v,
            SchemaValue::Map {
                entries: vec![(
                    SchemaValue::String("k".into()),
                    SchemaValue::String("v".into())
                )]
            }
        );
    }

    #[test]
    fn fixed_list_of_matching_length() {
        let v = literal_to_schema_value(
            &graph(SchemaType::fixed_list(SchemaType::u32(), 2)),
            &ToolLiteral::List(vec![ToolLiteral::Int(1), ToolLiteral::Int(2)]),
        )
        .unwrap();
        assert_eq!(
            v,
            SchemaValue::FixedList {
                elements: vec![SchemaValue::U32(1), SchemaValue::U32(2)]
            }
        );
    }

    #[test]
    fn fixed_list_of_wrong_length_is_rejected() {
        let err = literal_to_schema_value(
            &graph(SchemaType::fixed_list(SchemaType::u32(), 2)),
            &ToolLiteral::List(vec![ToolLiteral::Int(1)]),
        );
        assert!(err.is_err());
    }
}
