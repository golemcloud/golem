use crate::type_parameter::TypeParameter;
use crate::{Expr, InferredType};
use std::collections::VecDeque;

pub fn infer_worker_function_invokes(expr: &mut Expr) -> Result<(), String> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        if let Expr::Invoke {
            lhs,
            function_name,
            generic_type_parameter,
            args, ..
        } = expr
        {
            // There is no guarantee lhs is inferred during this time
            let inferred_type = lhs.clone().inferred_type(); // By this time we assume we correctly tag the inferred type of lhs to be InstanceType

            match inferred_type {
                InferredType::Instance { instance_type } => {
                    let generic_type_parameter = generic_type_parameter
                        .clone()
                        .map(|gtp| TypeParameter::from_str(&gtp.value))
                        .transpose()?;

                    let function =
                        instance_type.get_function(function_name, generic_type_parameter)?;
                    let function_name = function.dynamic_parsed_function_name()?;

                    let new_call = Expr::call(function_name, None, args.clone());

                    *expr = new_call;
                }
                // This implies, none of the phase identified `lhs` to be an instance-type yet.
                // This would
                _ => {}
            }
        }
        expr.visit_children_mut_bottom_up(&mut queue);
    }

    Ok(())
}
