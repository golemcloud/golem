use crate::type_refinement::precise_types::*;
use crate::type_refinement::TypeRefinement;
use crate::{Expr, InferredType, TypeName};
use golem_wasm_ast::analysis::AnalysedType;
use std::fmt::Display;
use std::ops::Deref;

#[derive(Clone, Debug)]
pub struct TypeCheckError {
    pub details: Vec<String>,
    pub expected_type: AnalysedType,
    pub actual_type: InferredType,
}


impl TypeCheckError {
    pub fn with_message(&self, message: String) -> TypeCheckError {
        let mut new_messages: TypeCheckError = self.clone();
        new_messages.details.push(message);
        new_messages
    }
    pub fn new(
        expected_type: AnalysedType,
        actual_type: InferredType,
        message: Option<String>,
    ) -> Self {
        TypeCheckError {
            details: message.map(|x| vec![x]).unwrap_or_default(),
            expected_type,
            actual_type,
        }
    }
}

impl Display for TypeCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for detail in self.details.iter() {
            write!(f, "{}\n", detail)?;
        }

        let expected_type = TypeName::try_from(self.expected_type.clone())
            .map(|x| x.to_string())
            .unwrap_or_default();

        if self.actual_type.is_one_of() || self.actual_type.is_all_of() {
            write!(f, "Expected type `{}` ", &expected_type)
        } else {
            write!(
                f,
                "Expected type `{}`, got `{:?}`",
                &expected_type, self.actual_type
            )
        }
    }
}


pub fn validate(
    expected_type: &AnalysedType,
    actual_type: &InferredType,
    actual_expr: &Expr,
) -> Result<(), TypeCheckError> {
    let un_inferred = check_unresolved_types(actual_expr);
    if let Err(msg) = un_inferred {
        return Err(TypeCheckError::new(
            expected_type.clone(),
            actual_type.clone(),
            Some(msg),
        ));
    } else {
        check_type_mismatch(expected_type, actual_type)
    }
}

pub fn check_unresolved_types(
     expr: &Expr,
) -> Result<(), String> {

    match expr {
        Expr::Record(record, _) => {
            internal::unresolved_types_in_record(&record.iter().map(|(k, v)| (k.clone(), v.deref().clone())).collect())
        }

        _ => Ok(())
    }
}

pub fn check_type_mismatch(
    expected_type: &AnalysedType,
    actual_type: &InferredType,
) -> Result<(), TypeCheckError> {
    match &expected_type {
        AnalysedType::Record(expected_type_record) => {
                let resolved = RecordType::refine(&actual_type);
                let expected_fields = expected_type_record.clone();
                match resolved {
                    Some(actual_record_type) => {
                        for expected_name_type_pair in expected_fields.fields {
                            let expected_field_name = expected_name_type_pair.name.clone();
                            let expected_field_type = expected_name_type_pair.typ.clone();
                            let actual_field_type = actual_record_type.inner_type_by_name(&expected_field_name);

                            if actual_field_type.is_unknown() {
                                return Err(TypeCheckError::new(
                                    expected_type.clone(),
                                    actual_type.clone(),
                                    Some(format!("Missing field {}", expected_field_name)),
                                ));
                            }

                            let result =
                                check_type_mismatch(&expected_field_type, &actual_field_type);

                            match result {
                                Ok(_) => {}
                                Err(e) => {
                                    return Err(e.with_message(format!(
                                        "Invalid type for field `{}`",
                                        expected_field_name
                                    )));
                                }
                            }
                        }

                        Ok(())
                    }

                    None => Err(TypeCheckError::new(
                        expected_type.clone(),
                        actual_type.clone(),
                        None,
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
        | AnalysedType::F64(_) => {
            let resolved = NumberType::refine(&actual_type);

            if let Some(_) = resolved {
                Ok(())
            } else {
                Err(TypeCheckError::new(
                    expected_type.clone(),
                    actual_type.clone(),
                    None,
                ))
            }
        }

        AnalysedType::Chr(_) => {
            let resolved = CharType::refine(&actual_type);

            if resolved.is_some() {
                Ok(())
            } else {
                Err(TypeCheckError::new(
                    expected_type.clone(),
                    actual_type.clone(),
                    None,
                ))
            }
        }

        AnalysedType::Variant(expected_variant) => {
            let actual_variant_type = VariantType::refine(&actual_type);

            match actual_variant_type {
                Some(actual_variant) => {
                    for expected_case in expected_variant.cases.iter() {
                        let expected_case_name = expected_case.name.clone();
                        let actual_case_type =
                            actual_variant.inner_type_by_name(&expected_case_name);

                        if let Some(expected_case_typ) = expected_case.typ.clone() {
                            let result = check_type_mismatch(&expected_case_typ, &actual_case_type);
                            match result {
                                Ok(_) => {}
                                Err(e) => {
                                    return Err(e.with_message(format!(
                                        "Invalid type for variant case `{}`",
                                        expected_case_name
                                    )));
                                }
                            }
                        }
                    }

                    Ok(())
                }

                None => {
                    return Err(TypeCheckError::new(
                        expected_type.clone(),
                        actual_type.clone(),
                        None,
                    ));
                }
            }
        }
        AnalysedType::Result(_) => {
            let actual_type_ok = OkType::refine(&actual_type).map(|t| t.inner_type().clone());
            let actual_type_err = ErrType::refine(&actual_type).map(|t| t.inner_type().clone());
            let expected = actual_type_ok.or(actual_type_err);

            if expected.is_some() {
                Ok(())
            } else {
                Err(TypeCheckError::new(
                    expected_type.clone(),
                    actual_type.clone(),
                    None,
                ))
            }
        }
        AnalysedType::Option(inner_type) => {
            let optional_type = OptionalType::refine(&actual_type).map(|t| t.inner_type().clone());

            if let Some(optional_type) = optional_type {
                check_type_mismatch(inner_type.inner.deref(), &optional_type)
            } else {
                Err(TypeCheckError::new(
                    expected_type.clone(),
                    actual_type.clone(),
                    None,
                ))
            }
        }

        AnalysedType::Enum(_) => {
            let actual_enum = EnumType::refine(&actual_type);

            if let Some(_) = actual_enum {
                Ok(())
            } else {
                Err(TypeCheckError::new(
                    expected_type.clone(),
                    actual_type.clone(),
                    None,
                ))
            }
        }
        AnalysedType::Flags(_) => {
            let actual_flags = FlagsType::refine(&actual_type);

            if let Some(_) = actual_flags {
                Ok(())
            } else {
                Err(TypeCheckError::new(
                    expected_type.clone(),
                    actual_type.clone(),
                    None,
                ))
            }
        }
        AnalysedType::Tuple(tuple) => {
            let actual_tuple = TupleType::refine(&actual_type);

            if let Some(actual_tuple) = actual_tuple {
                for (index, expected_type) in tuple.items.iter().enumerate() {
                    let actual_types = actual_tuple.inner_types();

                    let actual_types_vec = actual_types.into_iter().collect::<Vec<_>>();

                    let actual_type = actual_types_vec.get(index).ok_or(TypeCheckError::new(
                        expected_type.clone(),
                        actual_type.clone(),
                        Some("Actual tuple length is different".to_string()),
                    ))?;

                    let result = check_type_mismatch(expected_type, &actual_type);
                    match result {
                        Ok(_) => {}
                        Err(e) => {
                            return Err(e.with_message(format!(
                                "Invalid type for tuple item at index {}",
                                index
                            )));
                        }
                    }
                }

                Ok(())
            } else {
                Err(TypeCheckError::new(
                    expected_type.clone(),
                    actual_type.clone(),
                    None,
                ))
            }
        }
        AnalysedType::List(list_type) => {
            let actual_list = ListType::refine(&actual_type);

            if let Some(actual_list) = actual_list {
                let actual_inner_type = actual_list.inner_type().clone();
                let expected_inner_type = list_type.inner.deref().clone();
                let result = check_type_mismatch(&expected_inner_type, &actual_inner_type);
                match result {
                    Ok(_) => {}
                    Err(e) => {
                        return Err(e.with_message("Invalid type for list item".to_string()));
                    }
                }

                Ok(())
            } else {
                Err(TypeCheckError::new(
                    expected_type.clone(),
                    actual_type.clone(),
                    None,
                ))
            }
        }
        AnalysedType::Str(_) => {
            if let Some(_) = StringType::refine(&actual_type) {
                Ok(())
            } else {
                Err(TypeCheckError::new(
                    expected_type.clone(),
                    actual_type.clone(),
                    None,
                ))
            }
        }
        AnalysedType::Bool(_) => {
            if let Some(_) = BoolType::refine(&actual_type) {
                Ok(())
            } else {
                Err(TypeCheckError::new(
                    expected_type.clone(),
                    actual_type.clone(),
                    None,
                ))
            }
        }
        AnalysedType::Handle(_) => Ok(()),
    }
}

mod internal {
    use crate::{Expr, InferredType};
    use crate::type_checker::{check_unresolved_types, check_type_mismatch};

    pub fn unresolved_types_in_record(expr_fields: &Vec<(String, Expr)>) -> Result<(), String> {
        for (field_name, field_expr) in expr_fields {
            let field_type = field_expr.inferred_type();
            if field_type.is_unknown() || field_type.is_one_of() {
                return Err(format!("Un-inferred type for field `{}` in record", field_name))
            } else {
                check_unresolved_types(field_expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_types_in_tuple(expr_fields: &Vec<Expr>) -> Result<(), String> {
        for field_expr in expr_fields {
            let field_type = field_expr.inferred_type();
            if field_type.is_unknown() || field_type.is_one_of() {
                return Err("Un-inferred type for tuple item".to_string())
            } else {
                check_unresolved_types(field_expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_types_in_list(expr_fields: &Vec<Expr>) -> Result<(), String> {
        for field_expr in expr_fields {
            let field_type = field_expr.inferred_type();
            if field_type.is_unknown() || field_type.is_one_of() {
                return Err("Un-inferred type for list item".to_string())
            } else {
                check_unresolved_types(field_expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_types_in_variant(expr_fields: &Vec<(String, Option<Expr>)>) -> Result<(), String> {
        for (_, field_expr) in expr_fields {
            if let Some(field_expr) = field_expr {
                let field_type = field_expr.inferred_type();
                if field_type.is_unknown() || field_type.is_one_of() {
                    return Err("Un-inferred type for variant case".to_string())
                } else {
                    check_unresolved_types(field_expr)?;
                }
            }
        }

        Ok(())
    }


}
