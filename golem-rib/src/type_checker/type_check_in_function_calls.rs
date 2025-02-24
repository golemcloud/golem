use crate::call_type::CallType;
use crate::{Expr, FunctionCallError, FunctionTypeRegistry, RegistryKey};
use std::collections::VecDeque;

// While we have a dedicated generic phases (refer submodules) within type_checker module,
// we have this special phase to grab errors in the context function calls.
// This is grab as many errors as possible.
// Refer `FunctionCallTypeCheckError`.
#[allow(clippy::result_large_err)]
pub fn check_type_error_in_function_calls(
    expr: &mut Expr,
    type_registry: &FunctionTypeRegistry,
) -> Result<(), FunctionCallError> {
    let mut queue = VecDeque::new();

    queue.push_back(expr);

    while let Some(expr) = queue.pop_front() {
        let parent_copy = expr.clone();
        match expr {
            Expr::Call {
                call_type, args, ..
            } => match call_type {
                CallType::InstanceCreation(_) => {}
                call_type => internal::check_type_mismatch_in_function_call(
                    call_type,
                    args,
                    type_registry,
                    parent_copy,
                )?,
            },
            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }

    Ok(())
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
        function_call_expr: Expr, // The actual function call expression
    ) -> Result<(), FunctionCallError> {
        let registry_key = RegistryKey::from_call_type(call_type).ok_or(
            FunctionCallError::InvalidFunctionCall {
                function_call_name: call_type.to_string(),
                expr: function_call_expr.clone(),
                message: "invalid function call type".to_string(),
            },
        )?;

        let registry_value = type_registry.types.get(&registry_key).ok_or(
            FunctionCallError::InvalidFunctionCall {
                function_call_name: call_type.to_string(),
                expr: function_call_expr.clone(),
                message: "missing function in component metadata".to_string(),
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
                return Err(FunctionCallError::UnResolvedTypes {
                    function_call_name: call_type.to_string(),
                    argument: actual_arg.clone(),
                    unresolved_error,
                    expected_type: expected_arg_type.clone(),
                });
            }

            // Find possible missing fields in the arguments that are records
            let missing_fields =
                type_checker::find_missing_fields_in_record(actual_arg, &expected_arg_type);

            if !missing_fields.is_empty() {
                return Err(FunctionCallError::MissingRecordFields {
                    function_call_name: call_type.to_string(),
                    argument: actual_arg.clone(),
                    missing_fields,
                });
            }

            type_checker::check_type_mismatch(
                actual_arg,
                Some(&function_call_expr),
                &expected_arg_type,
                actual_arg_type,
            )
            .map_err(|e| FunctionCallError::TypeMisMatch {
                function_call_name: call_type.to_string(),
                argument: actual_arg.clone(),
                error: e,
            })?;
        }

        Ok(())
    }
}
