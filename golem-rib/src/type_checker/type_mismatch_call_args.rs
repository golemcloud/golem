use crate::call_type::CallType;
use crate::type_checker::{Path, TypeMismatchError};
use crate::{Expr, FunctionTypeRegistry, RegistryKey};
use golem_wasm_ast::analysis::AnalysedType;
use std::collections::VecDeque;
use std::fmt::Display;

pub fn check_type_mismatch_in_call_args(
    expr: &mut Expr,
    type_registry: &FunctionTypeRegistry,
) -> Result<(), FunctionCallTypeCheckError> {
    let mut queue = VecDeque::new();

    queue.push_back(expr);

    while let Some(expr) = queue.pop_front() {
        match expr {
            Expr::Call(call_type, args, ..) => {
                internal::check_type_mismatch_in_call_args(call_type, args, type_registry)?;
            }
            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }

    Ok(())
}

pub enum FunctionCallTypeCheckError {
    // This can hardly happen as we disallow further compilation
    // This can be removed in the earlier stages, and rely on type-check phase, if needed
    InvalidFunctionCallError {
        function_name: CallType,
    },
    TypeCheckError(TypeMismatchError),
    MissingRecordFieldsError {
        call_type: CallType,
        argument: Expr,
        missing_fields: Vec<Path>,
        _expected_type: AnalysedType,
    },
}

impl Display for FunctionCallTypeCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FunctionCallTypeCheckError::InvalidFunctionCallError { function_name } => {
                write!(
                    f,
                    "Function {} is not defined in the registry",
                    function_name
                )
            }
            FunctionCallTypeCheckError::TypeCheckError(error) => {
                write!(f, "{}", error)
            }
            FunctionCallTypeCheckError::MissingRecordFieldsError {
                call_type,
                argument,
                missing_fields,
                ..
            } => {
                let missing_fields = missing_fields
                    .iter()
                    .map(|path| path.to_string())
                    .collect::<Vec<String>>()
                    .join(", ");

                write!(
                    f,
                    "Invalid argument in `{}`: `{}`. Missing field `{}`",
                    call_type, argument, missing_fields
                )
            }
        }
    }
}

mod internal {
    use super::*;
    use crate::call_type::CallType;
    use crate::type_checker::{check_type_mismatch, Path, PathElem};

    use golem_wasm_ast::analysis::AnalysedType;

    pub(crate) fn check_type_mismatch_in_call_args(
        call_type: &mut CallType,
        args: &mut [Expr],
        type_registry: &FunctionTypeRegistry,
    ) -> Result<(), FunctionCallTypeCheckError> {
        let registry_value = type_registry
            .types
            .get(&RegistryKey::from_call_type(call_type))
            .ok_or(FunctionCallTypeCheckError::InvalidFunctionCallError {
                function_name: call_type.clone(),
            })?;

        let expected_arg_types = registry_value.argument_types();

        let mut filtered_expected_types = expected_arg_types.clone();

        if call_type.is_resource_method() {
            filtered_expected_types.remove(0);
        }

        for (actual_arg, expected_arg_type) in args.iter_mut().zip(filtered_expected_types) {
            let actual_arg_type = &actual_arg.inferred_type();

            let missing_fields = missing_fields_in_record(actual_arg, &expected_arg_type);

            if !missing_fields.is_empty() {
                return Err(FunctionCallTypeCheckError::MissingRecordFieldsError {
                    call_type: call_type.clone(),
                    argument: actual_arg.clone(),
                    missing_fields,
                    expected_type: expected_arg_type.clone(),
                });
            }

            check_type_mismatch(&expected_arg_type, &actual_arg_type)
                .map_err(|e| FunctionCallTypeCheckError::TypeCheckError(e))?;
        }

        Ok(())
    }

    fn missing_fields_in_record(expr: &Expr, expected: &AnalysedType) -> Vec<Path> {
        let mut missing_paths = Vec::new();

        if let AnalysedType::Record(record) = expected {
            for (field_name, analysed_type) in record
                .fields
                .iter()
                .map(|name_typ| (name_typ.name.clone(), name_typ.typ.clone()))
            {
                if let Expr::Record(record, _) = expr {
                    let value = record
                        .iter()
                        .find(|(name, _)| *name == field_name)
                        .map(|(_, value)| value);
                    if let Some(value) = value {
                        if let AnalysedType::Record(record) = analysed_type {
                            // Recursively check nested records
                            let nested_paths = missing_fields_in_record(
                                value,
                                &AnalysedType::Record(record.clone()),
                            );
                            for mut nested_path in nested_paths {
                                // Prepend the current field to the path for each missing nested field
                                nested_path.push_front(PathElem::Field(field_name.clone()));
                                missing_paths.push(nested_path);
                            }
                        }
                    } else {
                        missing_paths.push(Path::from_elem(PathElem::Field(field_name.clone())));
                    }
                }
            }
        }

        missing_paths
    }
}
