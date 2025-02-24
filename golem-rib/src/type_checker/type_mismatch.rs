use crate::type_inference::kind::TypeKind;
use crate::type_refinement::precise_types::*;
use crate::type_refinement::TypeRefinement;
use crate::{Expr, InferredType, Path, PathElem};
use golem_wasm_ast::analysis::AnalysedType;
use std::ops::Deref;

pub fn check_type_mismatch(
    expr: &Expr,
    parent_expr: Option<&Expr>,
    expected_type: &AnalysedType,
    actual_type: &InferredType,
) -> Result<(), TypeMismatchError> {
    match &expected_type {
        AnalysedType::Record(expected_type_record) => {
            let resolved = RecordType::refine(actual_type);
            let expected_fields = expected_type_record.clone();
            match resolved {
                Some(actual_record_type) => {
                    for expected_name_type_pair in expected_fields.fields {
                        let expected_field_name = expected_name_type_pair.name.clone();
                        let expected_field_type = expected_name_type_pair.typ.clone();
                        let actual_field_type =
                            actual_record_type.inner_type_by_name(&expected_field_name);

                        check_type_mismatch(
                            expr,
                            parent_expr,
                            &expected_field_type,
                            &actual_field_type,
                        )
                        .map_err(|e| e.at_field(expected_field_name.clone()))?;
                    }

                    Ok(())
                }

                None => Err(TypeMismatchError::with_actual_inferred_type(
                    expr,
                    parent_expr,
                    expected_type.clone(),
                    actual_type.clone(),
                )),
            }
        }

        AnalysedType::S8(_)
        | AnalysedType::S16(_)
        | AnalysedType::S32(_)
        | AnalysedType::S64(_)
        | AnalysedType::U8(_)
        | AnalysedType::U16(_)
        | AnalysedType::U32(_)
        | AnalysedType::U64(_)
        | AnalysedType::F32(_)
        | AnalysedType::F64(_) => NumberType::refine(actual_type).map(|_| ()).ok_or(
            TypeMismatchError::with_actual_inferred_type(
                expr,
                parent_expr,
                expected_type.clone(),
                actual_type.clone(),
            ),
        ),

        AnalysedType::Chr(_) => CharType::refine(actual_type).map(|_| ()).ok_or(
            TypeMismatchError::with_actual_inferred_type(
                expr,
                parent_expr,
                expected_type.clone(),
                actual_type.clone(),
            ),
        ),

        AnalysedType::Variant(expected_variant) => {
            let actual_variant_type = VariantType::refine(actual_type);

            match actual_variant_type {
                Some(actual_variant) => {
                    for expected_case in expected_variant.cases.iter() {
                        let expected_case_name = expected_case.name.clone();
                        let actual_case_type =
                            actual_variant.inner_type_by_name(&expected_case_name);

                        if let Some(expected_case_typ) = expected_case.typ.clone() {
                            check_type_mismatch(
                                expr,
                                parent_expr,
                                &expected_case_typ,
                                &actual_case_type,
                            )?;
                        }
                    }

                    Ok(())
                }

                None => Err(TypeMismatchError::with_actual_inferred_type(
                    expr,
                    parent_expr,
                    expected_type.clone(),
                    actual_type.clone(),
                )),
            }
        }
        AnalysedType::Result(type_result) => {
            let actual_type_ok = OkType::refine(actual_type).map(|t| t.inner_type().clone());
            let actual_type_err = ErrType::refine(actual_type).map(|t| t.inner_type().clone());

            let mut is_ok = false;

            if let (Some(actual_type_ok), Some(expected_type_ok)) =
                (actual_type_ok, type_result.ok.clone())
            {
                is_ok = true;
                check_type_mismatch(expr, parent_expr, &expected_type_ok, &actual_type_ok)?;
            }

            if let (Some(actual_type_err), Some(expected_type_err)) =
                (actual_type_err, type_result.err.clone())
            {
                check_type_mismatch(expr, parent_expr, &expected_type_err, &actual_type_err)?;
            } else {
                // Implies it the actual type is neither type of `ok`, nor the typ of `err`.
                // The complexity of the code is due to the fact that the actual type can be either `ok` or `err` or neither.
                if !is_ok {
                    return Err(TypeMismatchError::with_actual_inferred_type(
                        expr,
                        parent_expr,
                        expected_type.clone(),
                        actual_type.clone(),
                    ));
                }
            }

            Ok(())
        }
        AnalysedType::Option(inner_type) => {
            let optional_type = OptionalType::refine(actual_type).map(|t| t.inner_type().clone());

            if let Some(optional_type) = optional_type {
                check_type_mismatch(expr, parent_expr, inner_type.inner.deref(), &optional_type)
            } else {
                Err(TypeMismatchError::with_actual_inferred_type(
                    expr,
                    parent_expr,
                    expected_type.clone(),
                    actual_type.clone(),
                ))
            }
        }

        AnalysedType::Enum(_) => {
            let actual_enum = EnumType::refine(actual_type);

            if actual_enum.is_some() {
                Ok(())
            } else {
                Err(TypeMismatchError::with_actual_inferred_type(
                    expr,
                    parent_expr,
                    expected_type.clone(),
                    actual_type.clone(),
                ))
            }
        }
        AnalysedType::Flags(_) => FlagsType::refine(actual_type).map(|_| ()).ok_or(
            TypeMismatchError::with_actual_inferred_type(
                expr,
                parent_expr,
                expected_type.clone(),
                actual_type.clone(),
            ),
        ),
        AnalysedType::Tuple(tuple) => {
            let actual_tuple = TupleType::refine(actual_type);

            if let Some(actual_tuple) = actual_tuple {
                for (index, expected_type) in tuple.items.iter().enumerate() {
                    let actual_types = actual_tuple.inner_types();

                    let actual_types_vec = actual_types.into_iter().collect::<Vec<_>>();

                    let actual_type = actual_types_vec.get(index).ok_or(
                        TypeMismatchError::with_actual_inferred_type(
                            expr,
                            parent_expr,
                            expected_type.clone(),
                            actual_type.clone(),
                        ),
                    )?;

                    check_type_mismatch(expr, parent_expr, expected_type, actual_type)?;
                }

                Ok(())
            } else {
                Err(TypeMismatchError::with_actual_inferred_type(
                    expr,
                    parent_expr,
                    expected_type.clone(),
                    actual_type.clone(),
                ))
            }
        }
        AnalysedType::List(list_type) => {
            let actual_list = ListType::refine(actual_type);

            if let Some(actual_list) = actual_list {
                let actual_inner_type = actual_list.inner_type().clone();
                let expected_inner_type = list_type.inner.deref().clone();
                check_type_mismatch(expr, parent_expr, &expected_inner_type, &actual_inner_type)
                    .map_err(|e| e.updated_expected_type(&AnalysedType::List(list_type.clone())))
            } else {
                Err(TypeMismatchError::with_actual_inferred_type(
                    expr,
                    parent_expr,
                    expected_type.clone(),
                    actual_type.clone(),
                ))
            }
        }
        AnalysedType::Str(_) => StringType::refine(actual_type).map(|_| ()).ok_or(
            TypeMismatchError::with_actual_inferred_type(
                expr,
                parent_expr,
                expected_type.clone(),
                actual_type.clone(),
            ),
        ),
        AnalysedType::Bool(_) => BoolType::refine(actual_type).map(|_| ()).ok_or(
            TypeMismatchError::with_actual_inferred_type(
                expr,
                parent_expr,
                expected_type.clone(),
                actual_type.clone(),
            ),
        ),
        AnalysedType::Handle(_) => Ok(()),
    }
}

#[derive(Clone, Debug)]
pub struct TypeMismatchError {
    pub expr_with_wrong_type: Expr,
    pub parent_expr: Option<Expr>,
    pub expected_type: ExpectedType,
    pub actual_type: ActualType,
    pub field_path: Path,
}

#[derive(Clone, Debug)]
pub enum ExpectedType {
    AnalysedType(AnalysedType),
    Kind(TypeKind),
}

// If the actual type is not fully known but only a hint through TypeKind
#[derive(Clone, Debug)]
pub enum ActualType {
    Inferred(InferredType),
    Kind(TypeKind),
}

impl TypeMismatchError {
    pub fn updated_expected_type(&self, expected_type: &AnalysedType) -> TypeMismatchError {
        let mut mismatch_error: TypeMismatchError = self.clone();
        mismatch_error.expected_type = ExpectedType::AnalysedType(expected_type.clone());
        mismatch_error
    }

    pub fn at_field(&self, field_name: String) -> TypeMismatchError {
        let mut mismatch_error: TypeMismatchError = self.clone();
        mismatch_error
            .field_path
            .push_front(PathElem::Field(field_name));
        mismatch_error
    }

    pub fn with_actual_inferred_type(
        expr: &Expr,
        parent_expr: Option<&Expr>,
        expected_type: AnalysedType,
        actual_type: InferredType,
    ) -> Self {
        TypeMismatchError {
            expr_with_wrong_type: expr.clone(),
            parent_expr: parent_expr.cloned(),
            expected_type: ExpectedType::AnalysedType(expected_type),
            actual_type: ActualType::Inferred(actual_type),
            field_path: Path::default(),
        }
    }

    pub fn with_actual_type_kind(
        expr: &Expr,
        parent_expr: Option<&Expr>,
        expected_type: AnalysedType,
        actual_type: &TypeKind,
    ) -> Self {
        TypeMismatchError {
            expr_with_wrong_type: expr.clone(),
            parent_expr: parent_expr.cloned(),
            expected_type: ExpectedType::AnalysedType(expected_type),
            actual_type: ActualType::Kind(actual_type.clone()),
            field_path: Path::default(),
        }
    }
}
