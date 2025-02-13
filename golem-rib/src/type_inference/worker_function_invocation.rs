use std::collections::VecDeque;
use crate::{Expr, FunctionTypeRegistry, InferredType};

pub fn infer_worker_function_invokes(expr: &mut Expr, function_type_registry: &FunctionTypeRegistry,) -> Result<(), String> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        if let Expr::Invoke {lhs, function_name, generic_type_parameter, args, inferred_type } = expr {
            queue.push_back(lhs); // This variable hardly lead to another nested call, yet safe to push them to the queue

            let inferred_type = lhs.inferred_type(); // By this time we assume we correctly tag the inferred type of lhs to be InstanceType

            match inferred_type {
                InferredType::Instance { instance_type } => {

                },
                inferred_type => return Err(format!("Expected instance type, found {:?}", inferred_type))
            }

        }
        expr.visit_children_mut_bottom_up(&mut queue);
    }

    Ok(())

}