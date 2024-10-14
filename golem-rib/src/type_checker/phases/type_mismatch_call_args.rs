use crate::{Expr, FunctionTypeRegistry, RegistryKey};
use std::collections::VecDeque;

pub fn check_type_mismatch_in_call_args(
    expr: &mut Expr,
    type_registry: &FunctionTypeRegistry,
) -> Result<(), String> {
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

mod internal {
    use super::*;
    use crate::call_type::CallType;
    use crate::type_checker::{check_type_mismatch, validate};

    pub(crate) fn check_type_mismatch_in_call_args(
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

        let mut filtered_expected_types = expected_arg_types.clone();

        if call_type.is_resource_method() {
            filtered_expected_types.remove(0);
        }

        for (actual_arg, expected_arg_type) in args.iter_mut().zip(filtered_expected_types) {
            let actual_arg_type = &actual_arg.inferred_type();

            validate(&expected_arg_type, actual_arg_type, actual_arg).map_err(|type_check_error| {
                format!(
                    "Invalid argument in `{}`: `{}`. {}",
                    call_type,
                    actual_arg.to_string(),
                    type_check_error
                )
            })?;
        }

        Ok(())
    }
}
