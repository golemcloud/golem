use crate::rib_type_error::RibTypeError;
use crate::{Expr, InferredType, TypeInternal};
use std::collections::VecDeque;
// This is more of an optional stage to help with a better
// DX for Rib users, while not affecting the type inference reliability.
// A standalone expression is something that doesn't influence push down or pull up phases, or
// doesn't require further scanning but may require a final pull up.
// This is not a risk because there is no way it can influence
// the type of the rest of the rib program as they are standing alone.
// Mostly this simply helps with the inference of the return value of a Rib script.
// If there is any possible mistake in assigning types, type_checker phase
// or unification phase will capture it.
// However, this phase may not be perfectly assigning a reasonable type to all types of literals in the program
pub fn infer_orphan_literals(expr: &mut Expr) -> Result<(), RibTypeError> {
    infer_number_literals(expr);

    match expr {
        Expr::ExprBlock { exprs, .. } => {
            for expr in exprs {
                pull_types_up_for_standalone_expr(expr)?
            }

            expr.pull_types_up()?;
        }

        expr => pull_types_up_for_standalone_expr(expr)?,
    }

    Ok(())
}

fn pull_types_up_for_standalone_expr(expr: &mut Expr) -> Result<(), RibTypeError> {
    match expr {
        Expr::Sequence { .. } => {
            expr.pull_types_up()?;
        }
        Expr::Range { .. } => {
            expr.pull_types_up()?;
        }
        Expr::Record { .. } => {
            expr.pull_types_up()?;
        }
        Expr::Tuple { .. } => {
            expr.pull_types_up()?;
        }
        Expr::Plus { .. } => {
            expr.pull_types_up()?;
        }
        Expr::Multiply { .. } => {
            expr.pull_types_up()?;
        }
        Expr::Minus { .. } => {
            expr.pull_types_up()?;
        }
        Expr::Divide { .. } => {
            expr.pull_types_up()?;
        }
        Expr::Cond { .. } => {
            expr.pull_types_up()?;
        }
        Expr::PatternMatch { match_arms, .. } => {
            for arm in match_arms {
                arm.arm_resolution_expr.pull_types_up()?;
            }
            expr.pull_types_up()?;
        }
        Expr::Option { .. } => {
            expr.pull_types_up()?;
        }
        Expr::Result { .. } => {
            expr.pull_types_up()?;
        }
        Expr::ListReduce { .. } => {
            expr.pull_types_up()?;
        }

        Expr::ListComprehension { .. } => {
            expr.pull_types_up()?;
        }
        Expr::Length { .. } => {}
        Expr::Unwrap { .. } => {}
        Expr::Throw { .. } => {}
        Expr::GetTag { .. } => {}
        Expr::Call { .. } => {}
        Expr::EqualTo { .. } => {}
        Expr::Literal { .. } => {}
        Expr::Number { .. } => {}
        Expr::Flags { .. } => {}
        Expr::Identifier { .. } => {}
        Expr::Boolean { .. } => {}
        Expr::Concat { .. } => {}
        // We skip for nested blocks. Users can always type annotate
        // if fails to infer
        Expr::ExprBlock { .. } => {}
        Expr::Not { .. } => {}
        Expr::GreaterThan { .. } => {}
        Expr::And { .. } => {}
        Expr::Or { .. } => {}
        Expr::GreaterThanOrEqualTo { .. } => {}
        Expr::LessThanOrEqualTo { .. } => {}
        Expr::Let { .. } => {}
        Expr::SelectField { .. } => {}
        Expr::SelectIndex { .. } => {}
        Expr::InvokeMethodLazy { .. } => {}
        Expr::LessThan { .. } => {}
    }

    Ok(())
}

fn infer_number_literals(expr: &mut Expr) {
    // The result of an entire rib script is probably the last value
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Let { .. } => {}
            Expr::Call { .. } => {}
            Expr::SelectField { .. } => {}
            Expr::SelectIndex { .. } => {}
            Expr::InvokeMethodLazy { .. } => {}
            Expr::Identifier { .. } => {}
            Expr::PatternMatch { match_arms, .. } => {
                for arm in match_arms {
                    queue.push_back(&mut arm.arm_resolution_expr)
                }
            }
            Expr::Number {
                number,
                inferred_type,
                ..
            } => {
                // If a number is unresolved
                if inferred_type.un_resolved() {
                    *inferred_type = InferredType::from(&number.value);
                }
            }

            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }
}
