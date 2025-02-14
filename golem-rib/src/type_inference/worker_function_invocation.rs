use crate::type_parameter::TypeParameter;
use crate::{Expr, InferredType, TypeName};
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
            // This should be an instance type if instance_type_binding phase has been run.
            let inferred_type = lhs.clone().inferred_type();

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
                inferred_type => {
                    return Err(format!(
                        "Invalid worker function invoke. Expected {} to be an instance type, found {}",
                        lhs, TypeName::try_from(inferred_type).map(|x| x.to_string()).unwrap_or("Unknown".to_string())
                    ));
                }
            }
        }
        expr.visit_children_mut_bottom_up(&mut queue);
    }

    Ok(())
}
