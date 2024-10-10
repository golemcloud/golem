use std::collections::VecDeque;
use crate::{Expr, FunctionTypeRegistry, RegistryKey};

fn check_call_args(
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
            _ => {}
        }
    }


    Ok(())
}

mod internal {
    use golem_wasm_ast::analysis::AnalysedType;
    use crate::call_type::CallType;
    use super::*;

    pub fn check_call_args(
        call_type: &mut CallType,
        args: &mut Vec<Expr>,
        type_registry: &FunctionTypeRegistry,
    ) -> Result<(), String> {

        let registry_value =
            type_registry.types.get(&RegistryKey::from_call_type(call_type)).ok_or(format!(
                "Function {} is not defined in the registry",
                call_type
            ))?;

        let expected_arg_types = registry_value.argument_types();

        for (arg, expected_arg_type) in args.iter_mut().zip(expected_arg_types) {
            let actual_arg_type = arg.inferred_type().unify()?;

            let actual_arg_analysed_type = AnalysedType::try_from(&actual_arg_type)?;

            if actual_arg_analysed_type != expected_arg_type {
                return Err(format!(
                    "Function {} expects argument of type {:?}, but argument of type {:?} was provided",
                    call_type,
                    expected_arg_type,
                    actual_arg_type
                ));
            }
        }

        Ok(())
    }
}