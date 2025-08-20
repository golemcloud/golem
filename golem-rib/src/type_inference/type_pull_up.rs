// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::rib_source_span::SourceSpan;
use crate::rib_type_error::RibTypeErrorInternal;
use crate::type_inference::type_hint::TypeHint;
use crate::type_refinement::precise_types::{ListType, RecordType};
use crate::type_refinement::TypeRefinement;
use crate::FunctionName;
use crate::{
    ActualType, ComponentDependencies, ExpectedType, FullyQualifiedResourceMethod, GetTypeHint,
    InferredType, InstanceIdentifier, InterfaceName, MatchArm, PackageName, Path, Range,
    TypeInternal, TypeMismatchError,
};
use crate::{CustomError, Expr, ExprVisitor};

pub fn type_pull_up(
    expr: &mut Expr,
    component_dependencies: &ComponentDependencies,
) -> Result<(), RibTypeErrorInternal> {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_front() {
        match expr {
            Expr::Tuple {
                exprs,
                inferred_type,
                ..
            } => {
                handle_tuple(exprs, inferred_type);
            }

            Expr::Identifier { .. } => {}

            Expr::Flags { .. } => {}

            Expr::InvokeMethodLazy {
                lhs,
                method,
                source_span,
                args,
                ..
            } => {
                let new_call = handle_residual_method_invokes(
                    lhs,
                    method,
                    source_span,
                    args,
                    component_dependencies,
                )?;
                *expr = new_call
            }

            Expr::SelectField {
                expr,
                field,
                inferred_type,
                ..
            } => {
                handle_select_field(expr, field, inferred_type)?;
            }

            Expr::SelectIndex {
                expr,
                index,
                inferred_type,
                ..
            } => {
                handle_select_index(expr, index, inferred_type)?;
            }

            Expr::Result {
                expr: Ok(expr),
                inferred_type,
                ..
            } => {
                handle_result_ok(expr, inferred_type);
            }

            Expr::Result {
                expr: Err(expr),
                inferred_type,
                ..
            } => {
                handle_result_error(expr, inferred_type);
            }

            Expr::Option {
                expr: Some(expr),
                inferred_type,
                ..
            } => {
                handle_option_some(expr, inferred_type);
            }

            Expr::Option { .. } => {}

            Expr::Cond {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                handle_if_else(lhs, rhs, inferred_type);
            }

            Expr::PatternMatch {
                match_arms,
                inferred_type,
                ..
            } => {
                handle_pattern_match(match_arms, inferred_type);
            }

            Expr::Concat { .. } => {}

            Expr::ExprBlock {
                exprs,
                inferred_type,
                ..
            } => {
                handle_multiple(exprs, inferred_type);
            }

            Expr::Not { .. } => {}
            Expr::GreaterThan { .. } => {}
            Expr::GreaterThanOrEqualTo { .. } => {}
            Expr::LessThanOrEqualTo { .. } => {}
            Expr::EqualTo { .. } => {}
            Expr::LessThan { .. } => {}

            Expr::Plus {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                *inferred_type = inferred_type
                    .merge(lhs.inferred_type())
                    .merge(rhs.inferred_type());
            }

            Expr::Minus {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                *inferred_type = inferred_type
                    .merge(lhs.inferred_type())
                    .merge(rhs.inferred_type());
            }

            Expr::Multiply {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                *inferred_type = inferred_type
                    .merge(lhs.inferred_type())
                    .merge(rhs.inferred_type());
            }

            Expr::Divide {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                *inferred_type = inferred_type
                    .merge(lhs.inferred_type())
                    .merge(rhs.inferred_type());
            }

            Expr::Let { .. } => {}

            Expr::Sequence {
                exprs,
                inferred_type,
                ..
            } => {
                handle_sequence(exprs, inferred_type);
            }

            Expr::Record {
                exprs,
                inferred_type,
                ..
            } => handle_record(exprs, inferred_type),

            Expr::Literal { .. } => {}
            Expr::Number { .. } => {}
            Expr::Boolean { .. } => {}
            Expr::And { .. } => {}
            Expr::Or { .. } => {}
            Expr::Call { .. } => {}
            Expr::Unwrap {
                expr,
                inferred_type,
                ..
            } => {
                *inferred_type = inferred_type.merge(expr.inferred_type());
            }
            Expr::Length { .. } => {}
            Expr::Throw { .. } => {}
            Expr::GenerateWorkerName { .. } => {}
            Expr::ListComprehension {
                yield_expr,
                inferred_type,
                ..
            } => {
                handle_list_comprehension(yield_expr, inferred_type);
            }

            Expr::GetTag {
                expr,
                inferred_type,
                ..
            } => {
                *inferred_type = inferred_type.merge(expr.inferred_type());
            }

            Expr::ListReduce {
                init_value_expr,
                inferred_type,
                ..
            } => {
                *inferred_type = inferred_type.merge(init_value_expr.inferred_type());
            }

            Expr::Range {
                range,
                inferred_type,
                ..
            } => {
                handle_range(range, inferred_type);
            }
        }
    }

    Ok(())
}

fn handle_list_comprehension(
    current_yield_expr: &Expr,
    current_comprehension_type: &mut InferredType,
) {
    let list_expr = InferredType::list(current_yield_expr.inferred_type());
    *current_comprehension_type = current_comprehension_type.merge(list_expr);
}

fn handle_tuple(tuple_elems: &[Expr], current_tuple_type: &mut InferredType) {
    let mut new_inferred_type = vec![];

    for current_tuple_elem in tuple_elems.iter() {
        new_inferred_type.push(current_tuple_elem.inferred_type());
    }

    let new_tuple_type = InferredType::tuple(new_inferred_type);

    *current_tuple_type = current_tuple_type.merge(new_tuple_type);
}

fn handle_select_field(
    select_from: &Expr,
    field: &str,
    current_field_type: &mut InferredType,
) -> Result<(), RibTypeErrorInternal> {
    let selection_field_type = get_inferred_type_of_selected_field(select_from, field)?;

    *current_field_type = current_field_type.merge(selection_field_type);

    Ok(())
}

fn handle_select_index(
    select_from: &Expr,
    index: &Expr,
    current_select_index_type: &mut InferredType,
) -> Result<(), RibTypeErrorInternal> {
    let selection_expr_inferred_type = select_from.inferred_type();

    // if select_from is not yet gone through any phase, we cannot guarantee
    // it is a list type, otherwise continue with the assumption that it is a record
    if !selection_expr_inferred_type.is_unknown() {
        let index_type = get_inferred_type_of_selection_dynamic(select_from, index)?;

        *current_select_index_type = current_select_index_type.merge(index_type);
    }

    Ok(())
}

fn handle_result_ok(ok_expr: &mut Expr, current_inferred_type: &mut InferredType) {
    let inferred_type_of_ok_expr = ok_expr.inferred_type();
    let result_type = InferredType::result(Some(inferred_type_of_ok_expr.clone()), None);
    *current_inferred_type = current_inferred_type.merge(result_type);
}

fn handle_result_error(error_expr: &Expr, current_inferred_type: &mut InferredType) {
    let inferred_type_of_error_expr = error_expr.inferred_type();
    let result_type = InferredType::result(None, Some(inferred_type_of_error_expr.clone()));

    *current_inferred_type = current_inferred_type.merge(result_type);
}

fn handle_option_some(some_expr: &Expr, inferred_type: &mut InferredType) {
    let inferred_type_of_some_expr = some_expr.inferred_type();
    let option_type = InferredType::option(inferred_type_of_some_expr);

    *inferred_type = inferred_type.merge(option_type);
}

fn handle_if_else(then_expr: &Expr, else_expr: &Expr, inferred_type: &mut InferredType) {
    let inferred_type_of_then_expr = then_expr.inferred_type();
    let inferred_type_of_else_expr = else_expr.inferred_type();

    *inferred_type =
        inferred_type.merge(inferred_type_of_then_expr.merge(inferred_type_of_else_expr));
}

pub fn handle_pattern_match(current_match_arms: &[MatchArm], inferred_type: &mut InferredType) {
    let mut arm_resolution_inferred_types = vec![];

    for arm in current_match_arms {
        let arm_inferred_type = arm.arm_resolution_expr.inferred_type();
        arm_resolution_inferred_types.push(arm_inferred_type);
    }

    let new_inferred_type = InferredType::all_of(arm_resolution_inferred_types);

    *inferred_type = inferred_type.merge(new_inferred_type)
}

fn handle_multiple(expr_block: &[Expr], inferred_type: &mut InferredType) {
    let new_inferred_type = expr_block.last().map(|x| x.inferred_type());

    if let Some(new_inferred_type) = new_inferred_type {
        *inferred_type = inferred_type.merge(new_inferred_type);
    }
}

fn handle_sequence(current_expr_list: &[Expr], current_inferred_type: &mut InferredType) {
    let mut new_inferred_type = vec![];

    for expr in current_expr_list.iter() {
        let new_type = expr.inferred_type();
        new_inferred_type.push(new_type);
    }

    if let Some(first_inferred_type) = new_inferred_type.first() {
        *current_inferred_type =
            current_inferred_type.merge(InferredType::list(first_inferred_type.clone()));
    }
}

fn handle_record(current_expr_list: &[(String, Box<Expr>)], record_type: &mut InferredType) {
    let mut field_and_types = vec![];

    for (field, expr) in current_expr_list.iter() {
        field_and_types.push((field.clone(), expr.inferred_type()));
    }
    *record_type = record_type.merge(InferredType::record(field_and_types));
}

fn handle_range(range: &Range, inferred_type: &mut InferredType) {
    match range {
        Range::Range { from, to } => {
            let rhs = to.inferred_type();

            let lhs = from.inferred_type();

            let new_inferred_type = InferredType::range(lhs, Some(rhs));

            *inferred_type = new_inferred_type;
        }
        Range::RangeInclusive { from, to } => {
            let rhs = to.inferred_type();

            let lhs = from.inferred_type();

            let new_inferred_type = InferredType::range(lhs, Some(rhs));

            *inferred_type = new_inferred_type;
        }
        Range::RangeFrom { from } => {
            let lhs = from.inferred_type();

            let new_inferred_type = InferredType::range(lhs, None);

            *inferred_type = new_inferred_type;
        }
    }
}

fn get_inferred_type_of_selected_field(
    select_from: &Expr,
    field: &str,
) -> Result<InferredType, RibTypeErrorInternal> {
    let select_from_inferred_type = select_from.inferred_type();

    let refined_record = RecordType::refine(&select_from_inferred_type).ok_or({
        TypeMismatchError {
            source_span: select_from.source_span(),
            expected_type: ExpectedType::Hint(TypeHint::Record(None)),
            actual_type: ActualType::Inferred(select_from_inferred_type.clone()),
            field_path: Path::default(),
            additional_error_detail: vec![
                format!("cannot select `{}` from `{}`", field, select_from,),
                format!("if `{}` is a function, pass arguments", field),
            ],
        }
    })?;

    Ok(refined_record.inner_type_by_name(field))
}

// This is not an ideal logic yet,
// but alternate solution requires refactoring which can be done later
fn handle_residual_method_invokes(
    lhs: &Expr,
    method: &str,
    source_span: &SourceSpan,
    args: &[Expr],
    component_dependencies: &ComponentDependencies,
) -> Result<Expr, RibTypeErrorInternal> {
    let possible_resource = lhs.inferred_type().internal_type().clone();

    match possible_resource {
        TypeInternal::Resource {
            name,
            owner,
            ..
        } => {

            let fully_qualified_resource_method = if let Some(owner) = owner {
                let owner_string : String = owner;
                let parts: Vec<&str> = owner_string.split('/').collect();
                let namespace_and_package = parts.first().map(|s| s.to_string());

                let namespace = namespace_and_package
                    .as_ref()
                    .and_then(|s| s.split(':').next())
                    .map(|s| s.to_string());
                let package_name = namespace_and_package
                    .as_ref()
                    .and_then(|s| s.split(':').nth(1))
                    .map(|s| s.to_string());

                let interface_name = parts.get(1).map(|s| s.to_string());

                FullyQualifiedResourceMethod {
                    package_name: namespace.map(|namespace| PackageName {
                        namespace,
                        package_name: package_name.unwrap(),
                        version: None,
                    }),
                    interface_name: interface_name.map(|name| InterfaceName {
                        name,
                        version: None,
                    }),
                    resource_name: name.clone().unwrap(),
                    method_name: method.to_string(),
                    static_function: false,
                }
            } else {
                FullyQualifiedResourceMethod {
                    package_name: None,
                    interface_name: None,
                    resource_name: name.clone().unwrap(),
                    method_name: method.to_string(),
                    static_function: false,
                }
            };

            let function_name = FunctionName::ResourceMethod(fully_qualified_resource_method.clone());
            let (key, function_type) =
                component_dependencies.get_function_type(&None, &function_name).unwrap();

            let inferred_type = if let Some(new_inferred_type) = function_type.return_type {
                new_inferred_type
            } else {
                InferredType::unit()
            };

            let dynamic_parsed_function_name = fully_qualified_resource_method
                .dynamic_parsed_function_name().unwrap();

            let variable_id = match &lhs {
                Expr::Identifier { variable_id, .. } => Some(variable_id),
                _ => None,
            };

            let new_call = Expr::call_worker_function(
                dynamic_parsed_function_name,
                None,
                Some(InstanceIdentifier::WitResource {
                    variable_id: variable_id.cloned(),
                    worker_name: None,
                    resource_name: name.unwrap().to_string()
                }),
                args.to_vec(),
                Some(key)
            )
                .with_inferred_type(inferred_type)
                .with_source_span(source_span.clone());

            Ok(new_call)
        }

        _ => {
            Err(CustomError {
                source_span: source_span.clone(),
                help_message: vec![],
                message: format!("invalid method invocation `{lhs}.{method}`. make sure `{lhs}` is defined and is a valid instance type (i.e, resource or worker)"),
            }.into())
        }
    }
}

fn get_inferred_type_of_selection_dynamic(
    select_from: &Expr,
    index: &Expr,
) -> Result<InferredType, RibTypeErrorInternal> {
    let select_from_type = select_from.inferred_type();
    let select_index_type = index.inferred_type();

    let refined_list = ListType::refine(&select_from_type).ok_or({
        TypeMismatchError {
            source_span: select_from.source_span(),
            expected_type: ExpectedType::Hint(TypeHint::List(None)),
            actual_type: ActualType::Inferred(select_from_type.clone()),
            field_path: Default::default(),
            additional_error_detail: vec![format!(
                "cannot get index {} from {} since it is not a list type. Found: {}",
                index,
                select_from,
                select_from_type.get_type_hint()
            )],
        }
    })?;

    let list_type = refined_list.inner_type();

    if select_index_type.contains_only_number() {
        Ok(list_type)
    } else {
        Ok(InferredType::list(list_type))
    }
}

#[cfg(test)]
mod type_pull_up_tests {
    use bigdecimal::BigDecimal;

    use test_r::test;

    use crate::call_type::CallType;
    use crate::function_name::DynamicParsedFunctionName;
    use crate::DynamicParsedFunctionReference::IndexedResourceMethod;
    use crate::ParsedFunctionSite::PackagedInterface;
    use crate::{ArmPattern, ComponentDependencies, Expr, InferredType, MatchArm, VariableId};

    #[test]
    pub fn test_pull_up_identifier() {
        let expr = "foo";
        let mut expr = Expr::from_text(expr).unwrap();
        expr.add_infer_type_mut(InferredType::string());
        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();
        assert_eq!(expr.inferred_type(), InferredType::string());
    }

    #[test]
    pub fn test_pull_up_for_select_field() {
        let record_identifier =
            Expr::identifier_global("foo", None).merge_inferred_type(InferredType::record(vec![(
                "foo".to_string(),
                InferredType::record(vec![("bar".to_string(), InferredType::u64())]),
            )]));
        let select_expr = Expr::select_field(record_identifier, "foo", None);
        let mut expr = Expr::select_field(select_expr, "bar", None);
        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();
        assert_eq!(expr.inferred_type(), InferredType::u64());
    }

    #[test]
    pub fn test_pull_up_for_select_index() {
        let identifier = Expr::identifier_global("foo", None)
            .merge_inferred_type(InferredType::list(InferredType::u64()));
        let mut expr = Expr::select_index(identifier.clone(), Expr::number(BigDecimal::from(0)));
        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();
        let expected = Expr::select_index(identifier, Expr::number(BigDecimal::from(0)))
            .merge_inferred_type(InferredType::u64());
        assert_eq!(expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_sequence() {
        let elems = vec![
            Expr::number_inferred(BigDecimal::from(1), None, InferredType::u64()),
            Expr::number_inferred(BigDecimal::from(2), None, InferredType::u64()),
        ];

        let mut expr =
            Expr::sequence(elems.clone(), None).with_inferred_type(InferredType::unknown());
        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();

        assert_eq!(
            expr,
            Expr::sequence(elems, None).with_inferred_type(InferredType::list(InferredType::u64()))
        );
    }

    #[test]
    pub fn test_pull_up_for_tuple() {
        let mut expr = Expr::tuple(vec![
            Expr::literal("foo"),
            Expr::number_inferred(BigDecimal::from(1), None, InferredType::u64()),
        ]);

        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();

        assert_eq!(
            expr.inferred_type(),
            InferredType::tuple(vec![InferredType::string(), InferredType::u64()])
        );
    }

    #[test]
    pub fn test_pull_up_for_record() {
        let elems = vec![
            (
                "foo".to_string(),
                Expr::number_inferred(BigDecimal::from(1), None, InferredType::u64()),
            ),
            (
                "bar".to_string(),
                Expr::number_inferred(BigDecimal::from(2), None, InferredType::u32()),
            ),
        ];
        let mut expr = Expr::record(elems.clone()).with_inferred_type(InferredType::record(vec![
            ("foo".to_string(), InferredType::unknown()),
            ("bar".to_string(), InferredType::unknown()),
        ]));

        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();

        assert_eq!(
            expr,
            Expr::record(elems).with_inferred_type(InferredType::all_of(vec![
                InferredType::record(vec![
                    ("foo".to_string(), InferredType::u64()),
                    ("bar".to_string(), InferredType::u32())
                ]),
                InferredType::record(vec![
                    ("foo".to_string(), InferredType::unknown()),
                    ("bar".to_string(), InferredType::unknown())
                ])
            ]))
        );
    }

    #[test]
    pub fn test_pull_up_for_concat() {
        let mut expr = Expr::concat(vec![Expr::literal("foo"), Expr::literal("bar")]);
        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();
        let expected = Expr::concat(vec![Expr::literal("foo"), Expr::literal("bar")])
            .with_inferred_type(InferredType::string());
        assert_eq!(expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_not() {
        let mut expr = Expr::not(Expr::boolean(true));
        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();
        assert_eq!(expr.inferred_type(), InferredType::bool());
    }

    #[test]
    pub fn test_pull_up_if_else() {
        let inner1 = Expr::identifier_global("foo", None)
            .merge_inferred_type(InferredType::list(InferredType::u64()));

        let select_index1 = Expr::select_index(inner1.clone(), Expr::number(BigDecimal::from(0)));
        let select_index2 = Expr::select_index(inner1, Expr::number(BigDecimal::from(1)));

        let inner2 = Expr::identifier_global("bar", None)
            .merge_inferred_type(InferredType::list(InferredType::u64()));

        let select_index3 = Expr::select_index(inner2.clone(), Expr::number(BigDecimal::from(0)));
        let select_index4 = Expr::select_index(inner2, Expr::number(BigDecimal::from(1)));

        let mut expr = Expr::cond(
            Expr::greater_than(select_index1.clone(), select_index2.clone()),
            select_index3.clone(),
            select_index4.clone(),
        );

        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();
        let expected = Expr::cond(
            Expr::greater_than(
                Expr::select_index(
                    Expr::identifier_global("foo", None)
                        .with_inferred_type(InferredType::list(InferredType::u64())),
                    Expr::number(BigDecimal::from(0)),
                )
                .with_inferred_type(InferredType::u64()),
                Expr::select_index(
                    Expr::identifier_global("foo", None)
                        .with_inferred_type(InferredType::list(InferredType::u64())),
                    Expr::number(BigDecimal::from(1)),
                )
                .with_inferred_type(InferredType::u64()),
            )
            .with_inferred_type(InferredType::bool()),
            Expr::select_index(
                Expr::identifier_global("bar", None)
                    .with_inferred_type(InferredType::list(InferredType::u64())),
                Expr::number(BigDecimal::from(0)),
            )
            .with_inferred_type(InferredType::u64()),
            Expr::select_index(
                Expr::identifier_global("bar", None)
                    .with_inferred_type(InferredType::list(InferredType::u64())),
                Expr::number(BigDecimal::from(1)),
            )
            .with_inferred_type(InferredType::u64()),
        )
        .with_inferred_type(InferredType::u64());
        assert_eq!(expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_greater_than() {
        let inner =
            Expr::identifier_global("foo", None).merge_inferred_type(InferredType::record(vec![
                ("bar".to_string(), InferredType::string()),
                ("baz".to_string(), InferredType::u64()),
            ]));

        let select_field1 = Expr::select_field(inner.clone(), "bar", None);
        let select_field2 = Expr::select_field(inner, "baz", None);
        let mut expr = Expr::greater_than(select_field1.clone(), select_field2.clone());

        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();

        let expected = Expr::greater_than(
            select_field1.merge_inferred_type(InferredType::string()),
            select_field2.merge_inferred_type(InferredType::u64()),
        )
        .merge_inferred_type(InferredType::bool());
        assert_eq!(expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_greater_than_or_equal_to() {
        let inner = Expr::identifier_global("foo", None)
            .merge_inferred_type(InferredType::list(InferredType::u64()));

        let select_index1 = Expr::select_index(inner.clone(), Expr::number(BigDecimal::from(0)));
        let select_index2 = Expr::select_index(inner, Expr::number(BigDecimal::from(1)));
        let mut expr = Expr::greater_than_or_equal_to(select_index1.clone(), select_index2.clone());

        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();

        let expected = Expr::greater_than_or_equal_to(
            select_index1.merge_inferred_type(InferredType::u64()),
            select_index2.merge_inferred_type(InferredType::u64()),
        )
        .merge_inferred_type(InferredType::bool());
        assert_eq!(expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_less_than_or_equal_to() {
        let record_type = InferredType::record(vec![
            ("bar".to_string(), InferredType::string()),
            ("baz".to_string(), InferredType::u64()),
        ]);

        let inner = Expr::identifier_global("foo", None)
            .merge_inferred_type(InferredType::list(record_type.clone()));

        let select_field_from_first = Expr::select_field(
            Expr::select_index(inner.clone(), Expr::number(BigDecimal::from(0))),
            "bar",
            None,
        );
        let select_field_from_second = Expr::select_field(
            Expr::select_index(inner.clone(), Expr::number(BigDecimal::from(1))),
            "baz",
            None,
        );
        let mut expr = Expr::less_than_or_equal_to(
            select_field_from_first.clone(),
            select_field_from_second.clone(),
        );

        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();

        let new_select_field_from_first = Expr::select_field(
            Expr::select_index(inner.clone(), Expr::number(BigDecimal::from(0)))
                .merge_inferred_type(record_type.clone()),
            "bar",
            None,
        )
        .merge_inferred_type(InferredType::string());

        let new_select_field_from_second = Expr::select_field(
            Expr::select_index(inner.clone(), Expr::number(BigDecimal::from(1)))
                .merge_inferred_type(record_type),
            "baz",
            None,
        )
        .merge_inferred_type(InferredType::u64());

        let expected =
            Expr::less_than_or_equal_to(new_select_field_from_first, new_select_field_from_second)
                .merge_inferred_type(InferredType::bool());

        assert_eq!(expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_equal_to() {
        let mut expr = Expr::equal_to(
            Expr::number(BigDecimal::from(1)),
            Expr::number(BigDecimal::from(2)),
        );
        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();
        assert_eq!(expr.inferred_type(), InferredType::bool());
    }

    #[test]
    pub fn test_pull_up_for_less_than() {
        let mut expr = Expr::less_than(
            Expr::number(BigDecimal::from(1)),
            Expr::number(BigDecimal::from(2)),
        );

        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();

        assert_eq!(expr.inferred_type(), InferredType::bool());
    }

    #[test]
    pub fn test_pull_up_for_call() {
        let mut expr = Expr::call_worker_function(
            DynamicParsedFunctionName::parse("global_fn").unwrap(),
            None,
            None,
            vec![Expr::number(BigDecimal::from(1))],
            None,
        );

        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();

        assert_eq!(expr.inferred_type(), InferredType::unknown());
    }

    #[test]
    pub fn test_pull_up_for_dynamic_call() {
        let rib = r#"
           let input = { foo: "afs", bar: "al" };
           golem:it/api.{cart(input.foo).checkout}()
        "#;

        let mut expr = Expr::from_text(rib).unwrap();
        let component_dependencies = ComponentDependencies::default();

        expr.infer_types_initial_phase(&component_dependencies, &vec![])
            .unwrap();
        expr.infer_all_identifiers();
        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();

        let expected = Expr::expr_block(vec![
            Expr::let_binding_with_variable_id(
                VariableId::local("input", 0),
                Expr::record(vec![
                    (
                        "foo".to_string(),
                        Expr::literal("afs").with_inferred_type(InferredType::string()),
                    ),
                    (
                        "bar".to_string(),
                        Expr::literal("al").with_inferred_type(InferredType::string()),
                    ),
                ])
                .with_inferred_type(InferredType::record(vec![
                    ("foo".to_string(), InferredType::string()),
                    ("bar".to_string(), InferredType::string()),
                ])),
                None,
            ),
            Expr::call(
                CallType::function_call(
                    DynamicParsedFunctionName {
                        site: PackagedInterface {
                            namespace: "golem".to_string(),
                            package: "it".to_string(),
                            interface: "api".to_string(),
                            version: None,
                        },
                        function: IndexedResourceMethod {
                            resource: "cart".to_string(),
                            resource_params: vec![Expr::select_field(
                                Expr::identifier_local("input", 0, None).with_inferred_type(
                                    InferredType::record(vec![
                                        ("foo".to_string(), InferredType::string()),
                                        ("bar".to_string(), InferredType::string()),
                                    ]),
                                ),
                                "foo",
                                None,
                            )
                            .with_inferred_type(InferredType::string())],
                            method: "checkout".to_string(),
                        },
                    },
                    None,
                ),
                None,
                vec![],
            ),
        ]);

        assert_eq!(expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_unwrap() {
        let mut number = Expr::number(BigDecimal::from(1));
        number.with_inferred_type_mut(InferredType::f64());
        let mut expr = Expr::option(Some(number)).unwrap();
        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();
        assert_eq!(
            expr.inferred_type(),
            InferredType::option(InferredType::f64())
        );
    }

    #[test]
    pub fn test_pull_up_for_tag() {
        let mut number = Expr::number(BigDecimal::from(1));
        number.with_inferred_type_mut(InferredType::f64());
        let mut expr = Expr::get_tag(Expr::option(Some(number)));
        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();
        assert_eq!(
            expr.inferred_type(),
            InferredType::option(InferredType::f64())
        );
    }

    #[test]
    pub fn test_pull_up_for_pattern_match() {
        let mut expr = Expr::pattern_match(
            Expr::select_field(
                Expr::identifier_global("foo", None).merge_inferred_type(InferredType::record(
                    vec![("bar".to_string(), InferredType::string())],
                )),
                "bar",
                None,
            ),
            vec![
                MatchArm {
                    arm_pattern: ArmPattern::Constructor(
                        "cons1".to_string(),
                        vec![ArmPattern::Literal(Box::new(Expr::select_field(
                            Expr::identifier_global("foo", None).merge_inferred_type(
                                InferredType::record(vec![(
                                    "bar".to_string(),
                                    InferredType::string(),
                                )]),
                            ),
                            "bar",
                            None,
                        )))],
                    ),
                    arm_resolution_expr: Box::new(Expr::select_field(
                        Expr::identifier_global("baz", None).merge_inferred_type(
                            InferredType::record(vec![("qux".to_string(), InferredType::string())]),
                        ),
                        "qux",
                        None,
                    )),
                },
                MatchArm {
                    arm_pattern: ArmPattern::Constructor(
                        "cons2".to_string(),
                        vec![ArmPattern::Literal(Box::new(Expr::select_field(
                            Expr::identifier_global("quux", None).merge_inferred_type(
                                InferredType::record(vec![(
                                    "corge".to_string(),
                                    InferredType::string(),
                                )]),
                            ),
                            "corge",
                            None,
                        )))],
                    ),
                    arm_resolution_expr: Box::new(Expr::select_field(
                        Expr::identifier_global("grault", None).merge_inferred_type(
                            InferredType::record(vec![(
                                "garply".to_string(),
                                InferredType::string(),
                            )]),
                        ),
                        "garply",
                        None,
                    )),
                },
            ],
        );

        expr.pull_types_up(&ComponentDependencies::default())
            .unwrap();

        let expected = internal::expected_pattern_match();

        assert_eq!(expr, expected);
    }

    mod internal {
        use crate::{ArmPattern, Expr, InferredType, MatchArm};

        pub(crate) fn expected_pattern_match() -> Expr {
            Expr::pattern_match(
                Expr::select_field(
                    Expr::identifier_global("foo", None).with_inferred_type(InferredType::record(
                        vec![("bar".to_string(), InferredType::string())],
                    )),
                    "bar",
                    None,
                )
                .with_inferred_type(InferredType::string()),
                vec![
                    MatchArm {
                        arm_pattern: ArmPattern::Constructor(
                            "cons1".to_string(),
                            vec![ArmPattern::Literal(Box::new(
                                Expr::select_field(
                                    Expr::identifier_global("foo", None).with_inferred_type(
                                        InferredType::record(vec![(
                                            "bar".to_string(),
                                            InferredType::string(),
                                        )]),
                                    ),
                                    "bar",
                                    None,
                                )
                                .with_inferred_type(InferredType::string()),
                            ))],
                        ),
                        arm_resolution_expr: Box::new(
                            Expr::select_field(
                                Expr::identifier_global("baz", None).with_inferred_type(
                                    InferredType::record(vec![(
                                        "qux".to_string(),
                                        InferredType::string(),
                                    )]),
                                ),
                                "qux",
                                None,
                            )
                            .with_inferred_type(InferredType::string()),
                        ),
                    },
                    MatchArm {
                        arm_pattern: ArmPattern::Constructor(
                            "cons2".to_string(),
                            vec![ArmPattern::Literal(Box::new(
                                Expr::select_field(
                                    Expr::identifier_global("quux", None).with_inferred_type(
                                        InferredType::record(vec![(
                                            "corge".to_string(),
                                            InferredType::string(),
                                        )]),
                                    ),
                                    "corge",
                                    None,
                                )
                                .with_inferred_type(InferredType::string()),
                            ))],
                        ),
                        arm_resolution_expr: Box::new(
                            Expr::select_field(
                                Expr::identifier_global("grault", None).with_inferred_type(
                                    InferredType::record(vec![(
                                        "garply".to_string(),
                                        InferredType::string(),
                                    )]),
                                ),
                                "garply",
                                None,
                            )
                            .with_inferred_type(InferredType::string()),
                        ),
                    },
                ],
            )
            .with_inferred_type(InferredType::string())
        }
    }
}
