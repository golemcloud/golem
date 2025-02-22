use crate::instance_type::InstanceType;
use crate::{Expr, InferredType};


// Note that this takes an entire rib program and not any invalid expression
pub fn check_invalid_program_return(rib_program: &Expr) -> Result<(), InvalidProgramReturn> {
    let inferred_type = rib_program.inferred_type();

    if let InferredType::Instance { instance_type, .. } = inferred_type {

        let expr = match rib_program {
            Expr::ExprBlock { exprs, .. } if exprs.len() > 0 => {
                exprs.last().unwrap()
            }
            expr => expr
        };

        return match *instance_type {
            InstanceType::Resource { .. } => {
                Err(InvalidProgramReturn {
                    return_expr: expr.clone(),
                    message: "program is invalid as it returns a resource constructor".to_string()
                })
            }

            _  => {
                Err(InvalidProgramReturn {
                    return_expr: expr.clone(),
                    message: "program is invalid as it returns a worker instance".to_string()
                })
            }
        };
    }

    Ok(())
}


pub struct InvalidProgramReturn {
    pub return_expr: Expr,
    pub message: String,
}