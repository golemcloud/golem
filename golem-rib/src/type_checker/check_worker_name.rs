use crate::call_type::{CallType, InstanceCreationType};
use crate::Expr;
use std::collections::VecDeque;

// Capture all worker name and see if they are resolved to a string type
pub fn check_worker_name(expr: &Expr) -> Result<(), String> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Call { call_type, .. } => match call_type {
                CallType::InstanceCreation(InstanceCreationType::Worker { worker_name }) => {
                    internal::check_worker_name(worker_name)?;
                }
                CallType::Function { worker, .. } => {
                    internal::check_worker_name(worker)?;
                }
                CallType::VariantConstructor(_) => {}
                CallType::EnumConstructor(_) => {}
                CallType::InstanceCreation(InstanceCreationType::Resource {
                    worker_name, ..
                }) => {
                    internal::check_worker_name(worker_name)?;
                }
            },
            _ => expr.visit_children_bottom_up(&mut queue),
        }
    }

    Ok(())
}

mod internal {
    use crate::type_refinement::precise_types::StringType;
    use crate::type_refinement::TypeRefinement;
    use crate::{Expr, TypeName};

    pub(crate) fn check_worker_name(worker_name: &Option<Box<Expr>>) -> Result<(), String> {
        match worker_name {
            None => {}
            Some(expr) => {
                let inferred_type = expr.inferred_type();
                let string_type = StringType::refine(&inferred_type);

                match string_type {
                    Some(_) => {}
                    None => {
                        let type_name = TypeName::try_from(inferred_type.clone())?;
                        return Err(format!(
                            "Worker name expression `{}` is invalid. Worker name must be of the type string. Obtained {}",
                            expr, type_name
                        ));
                    }
                }
            }
        }

        Ok(())
    }
}
