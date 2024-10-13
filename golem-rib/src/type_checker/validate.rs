use crate::type_refinement::precise_types::*;
use crate::type_refinement::TypeRefinement;
use crate::{InferredType, TypeName};
use golem_wasm_ast::analysis::AnalysedType;
use std::fmt::Display;
use std::ops::Deref;

pub struct TypeCheckError {
    pub message: Option<String>,
    pub expected_type: String,
    pub actual_type: InferredType,
}

impl TypeCheckError {
    pub fn new(
        expected_type: AnalysedType,
        actual_type: InferredType,
        message: Option<String>,
    ) -> Self {
        TypeCheckError {
            message,
            expected_type: TypeName::try_from(expected_type).map(|x| x.to_string()).unwrap_or("Unknown".to_string()),
            actual_type,
        }
    }
}

impl Display for TypeCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(message) = &self.message {
            write!(f, "{}", message)?;
        }

        if self.actual_type.is_one_of() || self.actual_type.is_all_of() {
            write!(f, "Expected type `{}", self.expected_type)
        } else {
            write!(
                f,
                "Expected type `{}`, got `{:?}`",
                self.expected_type, self.actual_type
            )
        }
    }
}

pub fn validate(
    expected_type: &AnalysedType,
    actual_type: &InferredType,
) -> Result<(), TypeCheckError> {
    match &expected_type {
        AnalysedType::Record(fields) => {
            let resolved = RecordType::refine(&actual_type);

            let cloned = fields.clone();
            match resolved {
                Some(record_type) => {
                    for field in cloned.fields {
                        let field_name = field.name.clone();
                        let expected_field_type = field.typ.clone();
                        let actual_field_type = record_type.inner_type_by_name(&field_name);
                        let result = validate(&expected_field_type, &actual_field_type);
                        match result {
                            Ok(_) => {}
                            Err(e) => {
                                return Err(TypeCheckError::new(
                                    expected_field_type,
                                    actual_field_type,
                                    Some(format!("Invalid type for field {}. ", field_name)),
                                ));
                            }
                        }
                    }

                    Ok(())
                }

                None => Err(TypeCheckError::new(
                    expected_type.clone(),
                    actual_type.clone(),
                    None
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
                    Some("Invalid number type".to_string()),
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
                            let result = validate(&expected_case_typ, &actual_case_type);
                            match result {
                                Ok(_) => {}
                                Err(e) => {
                                    return Err(TypeCheckError::new(
                                        expected_case_typ,
                                        actual_case_type,
                                        Some(format!(
                                            "Invalid type for variant case {}",
                                            expected_case_name
                                        )),
                                    ));
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
                validate(inner_type.inner.deref(), &optional_type)
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

                    let result = validate(expected_type, &actual_type);
                    match result {
                        Ok(_) => {}
                        Err(e) => {
                            return Err(TypeCheckError::new(
                                expected_type.clone(),
                                actual_type.clone(),
                                Some(format!("Invalid type for tuple index {}", index)),
                            ));
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
                let result = validate(&expected_inner_type, &actual_inner_type);
                match result {
                    Ok(_) => {}
                    Err(e) => {
                        return Err(TypeCheckError::new(
                            expected_inner_type,
                            actual_inner_type,
                            Some(format!("Invalid type for list")),
                        ));
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
            dbg!(actual_type.clone());
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
