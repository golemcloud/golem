// Copyright 2024-2025 Golem Cloud
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

use crate::type_checker::Path;
use crate::type_checker::PathElem;
use crate::{Expr, VariableId};
use crate::{ExprVisitor, InferredType};

#[derive(Clone, Debug)]
pub struct GlobalVariableTypeSpec {
    variable_id: VariableId,
    path: Path,
    inferred_type: InferredType,
}

impl GlobalVariableTypeSpec {
    // Constructs a new `GlobalVariableTypeSpec`, which associates a specific inferred type
    // with a global variable and its nested path.
    //
    // A path denotes access to nested fields within a variable, where each field
    // may be typed explicitly. For example:
    //   - A specification like `a.*` implies that all fields under `a` are of type `Str`.
    //   - Similarly, `a.b.*` indicates that all fields under `a.b` are of type `Str`.
    //
    // Paths are expected to reference at least one nested field.
    //
    // The type system enforces consistency across declared paths. If contradictory default
    // types are specified for the same or overlapping paths, a compilation error will occur.
    //
    // Parameters:
    // - `variable_name`: The name of the root global variable (e.g., `"a"` in `a.b.c`).
    // - `path`: A `Path` representing the sequence of nested fields from the root variable
    //            (e.g., `[b, c]` for `a.b.c`).
    // - `inferred_type`: The enforced type (e.g., `Str`, `U64`) for the value
    //                    located at the specified path.
    // Note that the inferred_type is applied only to the element that exists after the end of the `path`.
    // For example, if the path is `a.b` and the inferred type is `Str`, then the type of `a.b.c` will be `Str`
    // and not for `a.b`
    pub fn new(
        variable_name: &str,
        path: Path,
        inferred_type: InferredType,
    ) -> GlobalVariableTypeSpec {
        GlobalVariableTypeSpec {
            variable_id: VariableId::global(variable_name.to_string()),
            path,
            inferred_type,
        }
    }
}

/// Applies global variable type specifications to an expression tree.
///
/// Iterates through all provided `GlobalVariableTypeSpec` entries and overrides
/// types in the given expression accordingly, enforcing the specified types on
/// matching variable paths.
pub fn bind_global_variable_types(expr: &mut Expr, type_pecs: &Vec<GlobalVariableTypeSpec>) {
    for spec in type_pecs {
        override_type(expr, spec);
    }
}

fn override_type(expr: &mut Expr, type_spec: &GlobalVariableTypeSpec) {
    let mut visitor = ExprVisitor::bottom_up(expr);

    // The full path starts from the variable_id and goes through the `path`
    let full_path = {
        let mut p = type_spec.path.clone();
        p.push_front(PathElem::Field(type_spec.variable_id.to_string()));
        p
    };

    let mut current_path = full_path.clone();
    let mut previous_expr_ptr: Option<*const Expr> = None;

    while let Some(expr) = visitor.pop_front() {
        match expr {
            Expr::Identifier {
                variable_id,
                inferred_type,
                ..
            } => {
                if variable_id == &type_spec.variable_id {
                    current_path.progress();

                    if type_spec.path.is_empty() {
                        *inferred_type = type_spec.inferred_type.clone();
                        previous_expr_ptr = None;
                        current_path = full_path.clone();
                    } else {
                        previous_expr_ptr = Some(expr as *const _);
                    }
                } else {
                    previous_expr_ptr = None;
                    current_path = full_path.clone();
                }
            }

            Expr::SelectField {
                expr: inner_expr,
                field,
                inferred_type,
                ..
            } => {
                if let Some(prev_ptr) = previous_expr_ptr {
                    if (inner_expr.as_ref() as *const _) == prev_ptr {
                        if current_path.is_empty() {
                            *inferred_type = type_spec.inferred_type.clone();
                            previous_expr_ptr = None;
                            current_path = full_path.clone();
                        } else if current_path.current()
                            == Some(&PathElem::Field(field.to_string()))
                        {
                            current_path.progress();
                            previous_expr_ptr = Some(expr as *const _);
                        } else {
                            previous_expr_ptr = None;
                            current_path = full_path.clone();
                        }
                    } else {
                        previous_expr_ptr = None;
                        current_path = full_path.clone();
                    }
                }
            }

            _ => {
                previous_expr_ptr = None;
                current_path = full_path.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rib_source_span::SourceSpan;
    use crate::{FunctionTypeRegistry, Id, TypeName};
    use test_r::test;

    #[test]
    fn test_override_types_1() {
        let mut expr = Expr::from_text(
            r#"
            foo
        "#,
        )
        .unwrap();

        let type_spec = GlobalVariableTypeSpec {
            variable_id: VariableId::global("foo".to_string()),
            path: Path::default(),
            inferred_type: InferredType::string(),
        };

        expr.bind_global_variable_types(&vec![type_spec]);

        let expected =
            Expr::identifier_global("foo", None).with_inferred_type(InferredType::string());

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_override_types_2() {
        let mut expr = Expr::from_text(
            r#"
            foo.bar.baz
        "#,
        )
        .unwrap();

        let type_spec = GlobalVariableTypeSpec {
            variable_id: VariableId::global("foo".to_string()),
            path: Path::from_elems(vec!["bar"]),
            inferred_type: InferredType::string(),
        };

        expr.bind_global_variable_types(&vec![type_spec]);

        let expected = Expr::select_field(
            Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
            "baz",
            None,
        )
        .with_inferred_type(InferredType::string());

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_override_types_5() {
        let mut expr = Expr::from_text(
            r#"
             let res = foo.bar.user-id;
             let hello: u64 = foo.bar.number;
             hello
        "#,
        )
        .unwrap();

        let type_spec = GlobalVariableTypeSpec {
            variable_id: VariableId::global("foo".to_string()),
            path: Path::from_elems(vec!["bar"]),
            inferred_type: InferredType::string(),
        };

        expr.infer_types(&FunctionTypeRegistry::empty(), &vec![type_spec])
            .unwrap();

        let expected = Expr::expr_block(vec![
            Expr::Let {
                variable_id: VariableId::Local("res".to_string(), Some(Id(0))),
                type_annotation: None,
                expr: Box::new(
                    Expr::select_field(
                        Expr::select_field(
                            Expr::identifier_global("foo", None).with_inferred_type(
                                InferredType::record(vec![(
                                    "bar".to_string(),
                                    InferredType::record(vec![
                                        ("number".to_string(), InferredType::u64()),
                                        ("user-id".to_string(), InferredType::string()),
                                    ]),
                                )]),
                            ),
                            "bar",
                            None,
                        )
                        .with_inferred_type(InferredType::record(vec![
                            ("number".to_string(), InferredType::u64()),
                            ("user-id".to_string(), InferredType::string()),
                        ])),
                        "user-id",
                        None,
                    )
                    .with_inferred_type(InferredType::string()),
                ),
                inferred_type: InferredType::tuple(vec![]),
                source_span: SourceSpan::default(),
            },
            Expr::Let {
                variable_id: VariableId::Local("hello".to_string(), Some(Id(0))),
                type_annotation: Some(TypeName::U64),
                expr: Box::new(
                    Expr::select_field(
                        Expr::select_field(
                            Expr::identifier_global("foo", None).with_inferred_type(
                                InferredType::record(vec![(
                                    "bar".to_string(),
                                    InferredType::record(vec![
                                        ("number".to_string(), InferredType::u64()),
                                        ("user-id".to_string(), InferredType::string()),
                                    ]),
                                )]),
                            ),
                            "bar",
                            None,
                        )
                        .with_inferred_type(InferredType::record(vec![
                            ("number".to_string(), InferredType::u64()),
                            ("user-id".to_string(), InferredType::string()),
                        ])),
                        "number",
                        None,
                    )
                    .with_inferred_type(InferredType::u64()),
                ),
                inferred_type: InferredType::tuple(vec![]),
                source_span: SourceSpan::default(),
            },
            Expr::Identifier {
                variable_id: VariableId::Local("hello".to_string(), Some(Id(0))),
                type_annotation: None,
                inferred_type: InferredType::u64(),
                source_span: SourceSpan::default(),
            },
        ])
        .with_inferred_type(InferredType::u64());

        assert_eq!(expr, expected);
    }
}
