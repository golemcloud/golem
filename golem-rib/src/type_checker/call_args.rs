use crate::{Expr, FunctionTypeRegistry, RegistryKey};
use std::collections::VecDeque;
use std::fmt::{Display, Formatter};
use std::ops::Deref;
use golem_wasm_ast::analysis::AnalysedType;

pub fn check_call_args(
    expr: &mut Expr,
    type_registry: &FunctionTypeRegistry,
) -> Result<(), String> {
    let mut queue = VecDeque::new();

    queue.push_back(expr);

    while let Some(expr) = queue.pop_front() {
        match expr {
            Expr::Call(call_type, args, ..) => {
                internal::check_call_args(call_type, args, type_registry)?;
            }
            _ => expr.visit_children_mut_bottom_up(&mut queue)
        }
    }

    Ok(())
}

mod internal {
    use super::*;
    use crate::call_type::CallType;
    use golem_wasm_ast::analysis::AnalysedType;
    use poem_openapi::types::Type;
    use crate::InferredType;
    use crate::type_refinement::precise_types::{BoolType, CharType, EnumType, ErrType, FlagsType, ListType, NumberType, OkType, OptionalType, RecordType, StringType, TupleType, VariantType};
    use crate::type_refinement::TypeRefinement;

    pub(crate) fn check_call_args(
        call_type: &mut CallType,
        args: &mut Vec<Expr>,
        type_registry: &FunctionTypeRegistry,
    ) -> Result<(), String> {
        let registry_value = type_registry
            .types
            .get(&RegistryKey::from_call_type(call_type))
            .ok_or(format!(
                "Function {} is not defined in the registry",
                call_type
            ))?;

        let expected_arg_types = registry_value.argument_types();

        for (arg, expected_arg_type) in args.iter_mut().zip(expected_arg_types) {
           validate(&expected_arg_type, &arg.inferred_type()).map_err(|e| format!("`{}` has invalid argument`{}`: {}. Actual: {:?}", call_type, arg.to_string(), e, arg.inferred_type()))?;
        }

        Ok(())
    }

    fn validate(expected_analysed_type: &AnalysedType, actual_type: &InferredType) -> Result<(), String> {
        match &expected_analysed_type {
            AnalysedType::Record(fields) => {
                let resolved = RecordType::refine(&actual_type);

                let cloned = fields.clone();
                match resolved {
                    Some(record_type) =>  {
                        for field in cloned.fields {
                            let field_name = field.name.clone();
                            let expected_field_type = field.typ.clone();
                            let actual_field_type = record_type.inner_type_by_name(&field_name);
                            let result = validate(&expected_field_type, &actual_field_type);
                            match result {
                                Ok(_) => {}
                                Err(e) => {
                                    return Err(format!("Invalid type for field `{}` in the record. Expected {}. {}", field_name, PrettyAnalysedType(expected_field_type), e));
                                }
                            }
                        }

                        Ok(())
                    }

                    None => Err(format!("Expected record type {}", PrettyAnalysedType(expected_analysed_type.clone())))
                }

            }

            AnalysedType::S8(_) | AnalysedType::S16(_) |
            AnalysedType::S32(_) | AnalysedType::S64(_) |
            AnalysedType::U8(_) | AnalysedType::U16(_) |
            AnalysedType::U32(_) | AnalysedType::U64(_) | AnalysedType::F32(_) | AnalysedType::F64(_) => {
                dbg!(actual_type.clone());
                let resolved =  NumberType::refine(&actual_type);
                dbg!(resolved.clone());


                if let Some(_) = resolved {
                    Ok(())
                } else {
                    Err(format!("Expected s32 type, but got {:?}", actual_type))
                }
            }


            AnalysedType::Chr(_) => {
                let resolved =  CharType::refine(&actual_type);

                if resolved.is_some() {
                    Ok(())
                } else {
                    Err(format!("Expected char type, but got {:?}", actual_type))
                }
            }

            AnalysedType::Variant(expected_variant) => {
                let actual_variant_type = VariantType::refine(&actual_type);

                match actual_variant_type {
                    Some(actual_variant) => {
                        for expected_case in expected_variant.cases.iter() {
                            let expected_case_name = expected_case.name.clone();
                            let actual_case_type = actual_variant.inner_type_by_name(&expected_case_name);

                            if let Some(expected_case_typ) = expected_case.typ.clone() {
                                let result = validate(&expected_case_typ, &actual_case_type);
                                match result {
                                    Ok(_) => {}
                                    Err(e) => {
                                        return Err(format!("Invalid type for case `{}` in the variant. Expected {}. {}", expected_case_name, PrettyAnalysedType(expected_case_typ), e));
                                    }
                                }
                            }

                        }

                        Ok(())
                    }

                    None => {
                        return Err(format!("Expected variant type {}", PrettyAnalysedType(expected_analysed_type.clone())));
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
                    Err(format!("Expected result type {}", PrettyAnalysedType(expected_analysed_type.clone())))
                }
            }
            AnalysedType::Option(inner_type) => {
                let optional_type = OptionalType::refine(&actual_type).map(|t| t.inner_type().clone());

                if let Some(optional_type) = optional_type {
                    validate(inner_type.inner.deref(), &optional_type)
                } else {
                    Err(format!("Expected option type {}", PrettyAnalysedType(expected_analysed_type.clone())))
                }
            }

            AnalysedType::Enum(_) => {
                let actual_enum = EnumType::refine(&actual_type);

                if let Some(_) = actual_enum {
                    Ok(())
                } else {
                    Err(format!("Expected enum type {}", PrettyAnalysedType(expected_analysed_type.clone())))
                }
            }
            AnalysedType::Flags(_) => {
                let actual_flags = FlagsType::refine(&actual_type);

                if let Some(_) = actual_flags {
                    Ok(())
                } else {
                    Err(format!("Expected flags type {}", PrettyAnalysedType(expected_analysed_type.clone())))
                }
            }
            AnalysedType::Tuple(tuple) => {
                let actual_tuple = TupleType::refine(&actual_type);

                if let Some(actual_tuple) = actual_tuple {
                    for (index, expected_type) in tuple.items.iter().enumerate() {
                        let actual_types = actual_tuple.inner_types();

                        let actual_types_vec = actual_types.into_iter().collect::<Vec<_>>();

                        let actual_type = actual_types_vec.get(index).ok_or(format!("Tuple index out of bounds"))?;

                        let result = validate(expected_type, &actual_type);
                        match result {
                            Ok(_) => {}
                            Err(e) => {
                                return Err(format!("Invalid type for tuple index `{}`. Expected {}. {}", index, PrettyAnalysedType(expected_type.clone()), e));
                            }
                        }
                    }

                    Ok(())
                } else {
                    Err(format!("Expected tuple type {}", PrettyAnalysedType(expected_analysed_type.clone())))
                }
            }
            AnalysedType::List(list_type) => {
                let actual_list = ListType::refine(&actual_type);

                if let Some(actual_list) = actual_list {
                    let actual_inner_type = actual_list.inner_type().clone();
                    let result = validate(&list_type.inner.deref().clone(), &actual_inner_type);
                    match result {
                        Ok(_) => {}
                        Err(e) => {
                            return Err(format!("Invalid type for list. Expected {}. {}", PrettyAnalysedType(list_type.inner.deref().clone()), e));
                        }
                    }

                    Ok(())
                } else {
                    Err(format!("Expected list type {}", PrettyAnalysedType(expected_analysed_type.clone())))
                }
            }
            AnalysedType::Str(_) => {
                dbg!(actual_type.clone());
                if let Some(_) = StringType::refine(&actual_type) {
                    Ok(())
                } else {
                    Err(format!("Expected str type, but got {:?}", actual_type))
                }
            }
            AnalysedType::Bool(_) => {
                if let Some(_) = BoolType::refine(&actual_type) {
                    Ok(())
                } else {
                    Err(format!("Expected bool type, but got {:?}", actual_type))
                }
            }
            AnalysedType::Handle(_) => {
                Ok(())
            }
        }

    }
}

pub struct PrettyAnalysedType(AnalysedType);

impl Display for PrettyAnalysedType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            AnalysedType::Record(fields) => {
                write!(f, "record {{")?;
                for field in fields.fields.iter() {
                    write!(f, "{}: {}, ", field.name, PrettyAnalysedType(field.clone().typ))?;
                }
                write!(f, "}}")
            }

            AnalysedType::S32(_) => write!(f, "s32"),
            AnalysedType::U64(_) => write!(f, "u64"),
            AnalysedType::Chr(_) => write!(f, "char"),
            AnalysedType::Result(type_result) => {
               let ok_type =
                   type_result.ok.clone().map(|t| PrettyAnalysedType(t.deref().clone())).map_or("unknown".to_string(), |t| t.to_string());

               let error_type =
                     type_result.err.clone().map(|t| PrettyAnalysedType(t.deref().clone())).map_or("unknown".to_string(), |t| t.to_string());

               write!(f, "Result<{}, {}>", ok_type, error_type)
           }
            AnalysedType::Option(t) => {
                let inner_type = PrettyAnalysedType(t.inner.deref().clone());
                write!(f, "Option<{}>", inner_type)
            }

            AnalysedType::Variant(type_variant) => {
                write!(f, "variant {{")?;
                for field in type_variant.cases.iter() {
                    let name = field.name.clone();
                    let typ =field.typ.clone();

                    match typ {
                        Some(t) => {
                            write!(f, "{}({}), ", name, PrettyAnalysedType(t))?;
                        }
                        None => {
                            write!(f, "{}, ", name)?;
                        }
                    }
                }
                write!(f, "}}")
            }
            AnalysedType::Enum(cases) => {
                write!(f, "enum {{")?;
                for case in cases.cases.iter() {
                    write!(f, "{}, ", case)?;
                }
                write!(f, "}}")
            }
            AnalysedType::Flags(flags) => {
                write!(f, "flags {{")?;
                for flag in flags.names.iter() {
                    write!(f, "{}, ", flag)?;
                }
                write!(f, "}}")
            }
            AnalysedType::Tuple(tuple) => {
                write!(f, "tuple<")?;
                for (index, typ) in tuple.items.iter().enumerate() {
                    write!(f, "{}", PrettyAnalysedType(typ.clone()))?;

                    if index < tuple.items.len() - 1 {
                        write!(f, ",")?;
                    }
                }
                write!(f, ">")
            }
            AnalysedType::List(list) => {
                write!(f, "list<{}>", PrettyAnalysedType(list.inner.deref().clone()))
            }
            AnalysedType::Str(_) => {
                write!(f, "str")
            }
            AnalysedType::F64(_) => {
                write!(f, "f64")
            }
            AnalysedType::F32(_) => {
                write!(f, "f32")
            }
            AnalysedType::S64(_) => {
                write!(f, "s64")
            }
            AnalysedType::U32(_) => {
                write!(f, "u32")
            }
            AnalysedType::U16(_) => {
                write!(f, "u16")
            }
            AnalysedType::S16(_) => {
                write!(f, "s16")
            }
            AnalysedType::U8(_) => {
                write!(f, "u8")
            }
            AnalysedType::S8(_) => {
                write!(f, "s8")
            }
            AnalysedType::Bool(_) => {
                write!(f, "bool")
            }
            AnalysedType::Handle(_) => {
                write!(f, "handle<>")
            }
        }
    }
}

#[cfg(test)]
mod type_check_tests {
    use super::*;
    use crate::compile;

    #[test]
    fn test_check_call_args() {
        let expr = r#"
          let result = foo({a: "foo", b: 2});
          result
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let metadata = internal::get_metadata();


        let result = compile(&expr, &metadata);

        dbg!(result.clone());

        assert!(false);
    }

    mod internal {
        use golem_wasm_ast::analysis::{AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult, AnalysedType, NameTypePair, TypeRecord};
        use golem_wasm_ast::analysis::analysed_type::{record, s32, str, u64};

        pub(crate) fn get_metadata() -> Vec<AnalysedExport> {
            let analysed_export = AnalysedExport::Function(
                AnalysedFunction {
                    name: "foo".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "arg1".to_string(),
                        typ: record(vec![
                            NameTypePair {
                                name: "a".to_string(),
                                typ: s32(),
                            },
                            NameTypePair {
                                name: "b".to_string(),
                                typ: u64(),
                            },
                        ]),
                    }],
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: str(),
                    }],
                }
            );

            vec![analysed_export]
        }
    }
}
