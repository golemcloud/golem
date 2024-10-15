use crate::call_type::CallType;
use crate::type_checker::{Path, TypeMismatchError, UnResolvedTypesError};
use crate::{Expr, FunctionTypeRegistry, RegistryKey, TypeName};
use golem_wasm_ast::analysis::AnalysedType;
use std::collections::VecDeque;
use std::fmt::Display;


// We grab as many errors as possible within the context of a function call
// to grab as many error details as possible. Refer `FunctionCallTypeCheckError` and therefore,
// this step is going to be step 1 of the type-checking process.
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
    TypeCheckError {
        call_type: CallType,
        argument: Expr,
        error: TypeMismatchError,
    },
    MissingRecordFieldsError {
        call_type: CallType,
        argument: Expr,
        missing_fields: Vec<Path>,
        expected_type: AnalysedType,
    },
    UnResolvedTypesError {
        call_type: CallType,
        argument: Expr,
        unresolved_error: UnResolvedTypesError,
        expected_type: AnalysedType,
    }
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
            FunctionCallTypeCheckError::TypeCheckError{call_type, argument, error} => {
                write!(
                    f,
                    "Invalid argument in `{}`: `{}`. {}",
                    call_type, argument, error
                )
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

            FunctionCallTypeCheckError::UnResolvedTypesError {
                call_type,
                argument,
                unresolved_error,
                expected_type,
            } => {
                write!(
                    f,
                    "Invalid argument in `{}`: `{}`. {}. Expected type: {}",
                    call_type, argument, unresolved_error, TypeName::try_from(expected_type.clone()).map(|t| t.to_string()).unwrap_or_default()
                )
            }
        }
    }
}

mod internal {
    use super::*;
    use crate::call_type::CallType;
    use crate::type_checker::{Path, PathElem};

    use golem_wasm_ast::analysis::AnalysedType;
    use crate::type_checker;

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

            // See if there are unresolved types in function arguments,
            // if so, tie them to the details specific to the function.
            // Finding resolved types can be called from anywhere, but this is called
            // within a function-call type-check phase,
            // to grab as many details as possible
            let unresolved_type =
                type_checker::find_unresolved_types(actual_arg);

            if let Err(unresolved_error) = unresolved_type {
                return Err(FunctionCallTypeCheckError::UnResolvedTypesError {
                    call_type: call_type.clone(),
                    argument: actual_arg.clone(),
                    unresolved_error,
                    expected_type: expected_arg_type.clone(),
                });
            }

            // Find possible missing fields in the arguments that are records
            let missing_fields =
                type_checker::find_missing_fields(actual_arg, &expected_arg_type);

            if !missing_fields.is_empty() {
                return Err(FunctionCallTypeCheckError::MissingRecordFieldsError {
                    call_type: call_type.clone(),
                    argument: actual_arg.clone(),
                    missing_fields,
                    expected_type: expected_arg_type.clone(),
                });
            }

            type_checker::check_type_mismatch(&expected_arg_type, &actual_arg_type)
                .map_err(|e| FunctionCallTypeCheckError::TypeCheckError{
                    call_type: call_type.clone(),
                    argument: actual_arg.clone(),
                    error: e,
                })?;
        }

        Ok(())
    }
}
