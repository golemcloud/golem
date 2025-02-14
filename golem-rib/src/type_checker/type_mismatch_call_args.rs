use crate::call_type::CallType;
use crate::type_checker::{Path, TypeMismatchError, UnResolvedTypesError};
use crate::{Expr, FunctionTypeRegistry, RegistryKey, TypeName};
use golem_wasm_ast::analysis::AnalysedType;
use std::collections::VecDeque;
use std::fmt::Display;

// While we have a dedicated generic phases (refer submodules) within type_checker module,
// we have this special phase to grab errors in the context function calls.
// This is grab as many errors as possible.
// Refer `FunctionCallTypeCheckError`.
#[allow(clippy::result_large_err)]
pub fn check_type_errors_in_function_call(
    expr: &mut Expr,
    type_registry: &FunctionTypeRegistry,
) -> Result<(), FunctionCallTypeError> {
    let mut queue = VecDeque::new();

    queue.push_back(expr);

    while let Some(expr) = queue.pop_front() {
        match expr {
            Expr::Call(call_type, _, args, ..) => match call_type {
                CallType::InstanceCreation(_) => {}
                call_type => {
                    internal::check_type_mismatch_in_function_call(call_type, args, type_registry)?
                }
            },
            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }

    Ok(())
}

pub enum FunctionCallTypeError {
    InvalidFunctionCall {
        function_call_name: String,
    },
    TypeMisMatch {
        function_call_name: String,
        argument: Expr,
        error: TypeMismatchError,
    },
    MissingRecordFields {
        function_call_name: String,
        argument: Expr,
        missing_fields: Vec<Path>,
    },
    UnResolvedTypes {
        function_call_name: String,
        argument: Expr,
        unresolved_error: UnResolvedTypesError,
        expected_type: AnalysedType,
    },
}

impl Display for FunctionCallTypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FunctionCallTypeError::InvalidFunctionCall {
                function_call_name: function_name,
            } => {
                write!(
                    f,
                    "Function {} is not defined in the registry",
                    function_name
                )
            }
            FunctionCallTypeError::TypeMisMatch {
                function_call_name: call_type,
                argument,
                error,
            } => {
                write!(
                    f,
                    "Invalid argument in `{}`: `{}`. {}",
                    call_type, argument, error
                )
            }
            FunctionCallTypeError::MissingRecordFields {
                function_call_name: call_type,
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

            FunctionCallTypeError::UnResolvedTypes {
                function_call_name: call_type,
                argument,
                unresolved_error,
                expected_type,
            } => {
                write!(
                    f,
                    "Invalid argument in `{}`: `{}`. Expected type: {}. {}",
                    call_type,
                    argument,
                    TypeName::try_from(expected_type.clone())
                        .map(|t| t.to_string())
                        .unwrap_or_default(),
                    unresolved_error
                )
            }
        }
    }
}

mod internal {
    use super::*;
    use crate::call_type::CallType;
    use crate::type_checker;

    #[allow(clippy::result_large_err)]
    pub(crate) fn check_type_mismatch_in_function_call(
        call_type: &mut CallType,
        args: &mut [Expr],
        type_registry: &FunctionTypeRegistry,
    ) -> Result<(), FunctionCallTypeError> {
        let registry_key = RegistryKey::from_call_type(call_type).ok_or(
            FunctionCallTypeError::InvalidFunctionCall {
                function_call_name: call_type.to_string(),
            },
        )?;

        let registry_value = type_registry.types.get(&registry_key).ok_or(
            FunctionCallTypeError::InvalidFunctionCall {
                function_call_name: call_type.to_string(),
            },
        )?;

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
            let unresolved_type = type_checker::check_unresolved_types(actual_arg);

            if let Err(unresolved_error) = unresolved_type {
                return Err(FunctionCallTypeError::UnResolvedTypes {
                    function_call_name: call_type.to_string(),
                    argument: actual_arg.clone(),
                    unresolved_error,
                    expected_type: expected_arg_type.clone(),
                });
            }

            // Find possible missing fields in the arguments that are records
            let missing_fields = type_checker::find_missing_fields(actual_arg, &expected_arg_type);

            if !missing_fields.is_empty() {
                return Err(FunctionCallTypeError::MissingRecordFields {
                    function_call_name: call_type.to_string(),
                    argument: actual_arg.clone(),
                    missing_fields,
                });
            }

            type_checker::check_type_mismatch(&expected_arg_type, actual_arg_type).map_err(
                |e| FunctionCallTypeError::TypeMisMatch {
                    function_call_name: call_type.to_string(),
                    argument: actual_arg.clone(),
                    error: e,
                },
            )?;
        }

        Ok(())
    }
}
