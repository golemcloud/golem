use crate::{Expr, InferredType};
use std::collections::{HashMap, VecDeque};

// This is about binding the `InstanceType` to the corresponding identifiers.
//
// Example:
//  let foo = instance("worker-name");
//  foo.bar("baz")
//  With this phase `foo` in `foo.bar("baz")` will have inferred type of `InstanceType`
//
// Note that this compilation phase should be after variable binding phases
// (where we assign identities to variables that ensuring scoping).
//
// Example:
//  let foo = instance("worker-name");
//  let foo = "bar";
//  foo
//
// In this case `foo` in `foo` should have inferred type of `String` and not `InstanceType`
pub fn bind_instance_types(expr: &mut Expr) {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    let mut instance_variables = HashMap::new();

    while let Some(expr) = queue.pop_front() {
        match expr {
            Expr::Let{ variable_id, expr, ..} => {
                if let InferredType::Instance { instance_type } = expr.inferred_type() {
                    instance_variables.insert(variable_id.clone(), instance_type);
                }

                queue.push_front(expr)
            }
            Expr::Identifier{ variable_id, inferred_type, ..} => {
                if let Some(new_inferred_type) = instance_variables.get(variable_id) {
                    *inferred_type = InferredType::Instance {
                        instance_type: new_inferred_type.clone(),
                    };
                }
            }

            _ => expr.visit_children_mut_top_down(&mut queue),
        }
    }
}
