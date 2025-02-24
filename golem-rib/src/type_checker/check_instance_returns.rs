use crate::instance_type::InstanceType;
use crate::{Expr, InferredType};

pub fn check_instance_returns(expr: &Expr) -> Result<(), String> {
    let inferred_type = expr.inferred_type();

    if let InferredType::Instance { instance_type, .. } = inferred_type {
        return match *instance_type {
            InstanceType::Resource { .. } => {
                Err("Resource constructor instance cannot be returned".to_string())
            }

            _ => Err("Worker instance cannot be returned".to_string()),
        };
    }

    Ok(())
}
