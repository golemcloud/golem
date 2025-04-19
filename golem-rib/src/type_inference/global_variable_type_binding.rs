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

use crate::rib_type_error::RibTypeError;
use crate::type_checker::Path;
use crate::{Expr, InferredType, VariableId};

// The goal is to be able to specify the types associated with an identifier.
// i.e, `a.*` is always `Str`, or `a.b.*` is always `Str`, or `a.b.c` is always `Str`
// This can be represented using `GlobalVariableTypeSpec { a, vec![], Str }`, `GlobalVariableTypeSpec {a, b, Str}`  and
// `GlobalVariableTypeSpec {a, vec[b, c], Str}` respectively
// If you specify completely opposite types to be default, you will get a compilation error.
#[derive(Clone, Debug)]
pub struct GlobalVariableTypeSpec {
    variable_id: VariableId,
    path: Path,
    inferred_type: InferredType,
}

impl GlobalVariableTypeSpec {
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

pub fn bind_global_variable_types(
    expr: &Expr,
    type_pecs: &Vec<GlobalVariableTypeSpec>,
) -> Result<Expr, RibTypeError> {
    let mut result_expr = expr.clone();

    for spec in type_pecs {
        internal::override_type(&mut result_expr, spec);
    }

    Ok(result_expr)
}

mod internal {
    use crate::type_checker::{Path, PathElem};
    use crate::{Expr, ExprVisitor, GlobalVariableTypeSpec};

    pub(crate) fn override_type(expr: &mut Expr, type_spec: &GlobalVariableTypeSpec) {
        let mut previous_expr: Option<Expr> = None;

        let mut visitor = ExprVisitor::bottom_up(expr);

        fn set_path(type_spec: &GlobalVariableTypeSpec) -> Path {
            let mut path = type_spec.path.clone();
            path.push_front(PathElem::Field(type_spec.variable_id.to_string()));
            path
        }

        let mut path = set_path(type_spec);

        while let Some(expr) = visitor.pop_front() {
            match expr {
                Expr::Identifier {
                    variable_id,
                    inferred_type,
                    ..
                } => {
                    if variable_id == &type_spec.variable_id {
                        path.progress();

                        if type_spec.path.is_empty() {
                            *inferred_type = type_spec.inferred_type.clone();
                        } else {
                            previous_expr = Some(expr.clone());
                        }
                    } else {
                        previous_expr = None;
                        path = set_path(type_spec);
                    }
                }

                Expr::SelectField {
                    expr: inner_expr,
                    field,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    if let Some(previous_identifier) = &previous_expr {
                        if inner_expr.as_ref() == previous_identifier {
                            if path.current() == Some(&PathElem::Field(field.to_string())) {
                                path.progress();

                                if path.is_empty() {
                                    *inferred_type = type_spec.inferred_type.clone();
                                } else {
                                    previous_expr = Some(Expr::SelectField {
                                        expr: inner_expr.clone(),
                                        field: field.clone(),
                                        type_annotation: type_annotation.clone(),
                                        inferred_type: type_spec.inferred_type.clone(),
                                        source_span: source_span.clone(),
                                    });
                                }
                            }
                        } else {
                            previous_expr = None;
                            path = set_path(type_spec);
                        }
                    }
                }

                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rib_source_span::SourceSpan;
    use crate::{ExprVisitor, FunctionTypeRegistry, Id, TypeName};
    use test_r::test;

    #[test]
    fn test_override_types_1() {
        let expr = Expr::from_text(
            r#"
            foo
        "#,
        )
        .unwrap();

        let type_spec = GlobalVariableTypeSpec {
            variable_id: VariableId::global("foo".to_string()),
            path: Path::default(),
            inferred_type: InferredType::Str,
        };

        let result = expr.bind_global_variable_types(&vec![type_spec]).unwrap();

        let expected = Expr::identifier_global("foo", None).with_inferred_type(InferredType::Str);

        assert_eq!(result, expected);
    }

    // Be able to
    #[test]
    fn test_override_types_2() {
        let expr = Expr::from_text(
            r#"
            foo.bar.baz
        "#,
        )
        .unwrap();

        let type_spec = GlobalVariableTypeSpec {
            variable_id: VariableId::global("foo".to_string()),
            path: Path::from_elems(vec!["bar"]),
            inferred_type: InferredType::Str,
        };

        let result = expr.bind_global_variable_types(&vec![type_spec]).unwrap();

        let expected = Expr::select_field(
            Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
            "baz",
            None,
        )
        .with_inferred_type(InferredType::Str);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_override_types_3() {
        let expr = Expr::from_text(
            r#"
            foo.bar.baz
        "#,
        )
        .unwrap();

        let type_spec = GlobalVariableTypeSpec {
            variable_id: VariableId::global("foo".to_string()),
            path: Path::from_elems(vec!["bar", "baz"]),
            inferred_type: InferredType::Str,
        };

        let result = expr.bind_global_variable_types(&vec![type_spec]).unwrap();

        let expected = Expr::select_field(
            Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
            "baz",
            None,
        )
        .with_inferred_type(InferredType::Str);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_override_types_4() {
        let expr = Expr::from_text(
            r#"
            foo.bar.baz
        "#,
        )
        .unwrap();

        let type_spec = GlobalVariableTypeSpec {
            variable_id: VariableId::global("foo".to_string()),
            path: Path::default(),
            inferred_type: InferredType::Str,
        };

        let result = expr.bind_global_variable_types(&vec![type_spec]).unwrap();

        let expected = Expr::select_field(
            Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
            "baz",
            None,
        )
        .with_inferred_type(InferredType::Str);

        assert_eq!(result, expected);
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
            inferred_type: InferredType::Str,
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
                                InferredType::Record(vec![(
                                    "bar".to_string(),
                                    InferredType::Record(vec![
                                        ("number".to_string(), InferredType::U64),
                                        ("user-id".to_string(), InferredType::Str),
                                    ]),
                                )]),
                            ),
                            "bar",
                            None,
                        )
                        .with_inferred_type(InferredType::Record(vec![
                            ("number".to_string(), InferredType::U64),
                            ("user-id".to_string(), InferredType::Str),
                        ])),
                        "user-id",
                        None,
                    )
                    .with_inferred_type(InferredType::Str),
                ),
                inferred_type: InferredType::Tuple(vec![]),
                source_span: SourceSpan::default(),
            },
            Expr::Let {
                variable_id: VariableId::Local("hello".to_string(), Some(Id(0))),
                type_annotation: Some(TypeName::U64),
                expr: Box::new(
                    Expr::select_field(
                        Expr::select_field(
                            Expr::identifier_global("foo", None).with_inferred_type(
                                InferredType::Record(vec![(
                                    "bar".to_string(),
                                    InferredType::Record(vec![
                                        ("number".to_string(), InferredType::U64),
                                        ("user-id".to_string(), InferredType::Str),
                                    ]),
                                )]),
                            ),
                            "bar",
                            None,
                        )
                        .with_inferred_type(InferredType::Record(vec![
                            ("number".to_string(), InferredType::U64),
                            ("user-id".to_string(), InferredType::Str),
                        ])),
                        "number",
                        None,
                    )
                    .with_inferred_type(InferredType::U64),
                ),
                inferred_type: InferredType::Tuple(vec![]),
                source_span: SourceSpan::default(),
            },
            Expr::Identifier {
                variable_id: VariableId::Local("hello".to_string(), Some(Id(0))),
                type_annotation: None,
                inferred_type: InferredType::U64,
                source_span: SourceSpan::default(),
            },
        ])
        .with_inferred_type(InferredType::U64);

        assert_eq!(expr, expected);
    }
}
