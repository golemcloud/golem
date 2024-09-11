use crate::Expr;
use std::collections::VecDeque;
use std::ops::Deref;

// Visits each children of the expression and push them to the back of the queue
pub fn visit_children_bottom_up_mut<'a>(expr: &'a mut Expr, queue: &mut VecDeque<&'a mut Expr>) {
    match expr {
        Expr::Let(_, _, expr, _) => queue.push_back(&mut *expr),
        Expr::SelectField(expr, _, _) => queue.push_back(&mut *expr),
        Expr::SelectIndex(expr, _, _) => queue.push_back(&mut *expr),
        Expr::Sequence(exprs, _) => queue.extend(exprs.iter_mut()),
        Expr::Record(exprs, _) => queue.extend(exprs.iter_mut().map(|(_, expr)| &mut **expr)),
        Expr::Tuple(exprs, _) => queue.extend(exprs.iter_mut()),
        Expr::Concat(exprs, _) => queue.extend(exprs.iter_mut()),
        Expr::Multiple(exprs, _) => queue.extend(exprs.iter_mut()), // let x = 1, y = call(x);
        Expr::Not(expr, _) => queue.push_back(&mut *expr),
        Expr::GreaterThan(lhs, rhs, _) => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs);
        }
        Expr::GreaterThanOrEqualTo(lhs, rhs, _) => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs);
        }
        Expr::LessThanOrEqualTo(lhs, rhs, _) => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs);
        }
        Expr::EqualTo(lhs, rhs, _) => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs);
        }
        Expr::LessThan(lhs, rhs, _) => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs);
        }
        Expr::Cond(cond, then, else_, _) => {
            queue.push_back(&mut *cond);
            queue.push_back(&mut *then);
            queue.push_back(&mut *else_);
        }
        Expr::PatternMatch(expr, arms, _) => {
            queue.push_back(&mut *expr);
            for arm in arms {
                let arm_literal_expressions = arm.arm_pattern.get_expr_literals_mut();
                queue.extend(arm_literal_expressions.into_iter().map(|x| x.as_mut()));
                queue.push_back(&mut *arm.arm_resolution_expr);
            }
        }
        Expr::Option(Some(expr), _) => queue.push_back(&mut *expr),
        Expr::Result(Ok(expr), _) => queue.push_back(&mut *expr),
        Expr::Result(Err(expr), _) => queue.push_back(&mut *expr),
        Expr::Call(_, expressions, _) => queue.extend(expressions.iter_mut()),
        Expr::Unwrap(expr, _) => queue.push_back(&mut *expr), // not yet needed
        Expr::Get(expr, _, _) => queue.push_back(&mut *expr),
        Expr::And(expr1, expr2, _) => {
            queue.push_back(&mut *expr1);
            queue.push_back(&mut *expr2)
        },
        Expr::Literal(_, _) => {}
        Expr::Number(_, _, _) => {}
        Expr::Flags(_, _) => {}
        Expr::Identifier(_, _) => {}
        Expr::Boolean(_, _) => {}
        Expr::Option(None, _) => {}
        Expr::Throw(_, _) => {}
        Expr::Tag(_, _) => {}
    }
}

pub fn visit_children_bottom_up<'a>(expr: &'a Expr, queue: &mut VecDeque<&'a Expr>) {
    match expr {
        Expr::Let(_, _, expr, _) => queue.push_back(expr),
        Expr::SelectField(expr, _, _) => queue.push_back(expr),
        Expr::SelectIndex(expr, _, _) => queue.push_back(expr),
        Expr::Sequence(exprs, _) => queue.extend(exprs.iter()),
        Expr::Record(exprs, _) => queue.extend(exprs.iter().map(|(_, expr)| expr.deref())),
        Expr::Tuple(exprs, _) => queue.extend(exprs.iter()),
        Expr::Concat(exprs, _) => queue.extend(exprs.iter()),
        Expr::Multiple(exprs, _) => queue.extend(exprs.iter()), // let x = 1, y = call(x);
        Expr::Not(expr, _) => queue.push_back(expr),
        Expr::GreaterThan(lhs, rhs, _) => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::GreaterThanOrEqualTo(lhs, rhs, _) => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::LessThanOrEqualTo(lhs, rhs, _) => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::EqualTo(lhs, rhs, _) => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::LessThan(lhs, rhs, _) => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::Cond(cond, then, else_, _) => {
            queue.push_back(cond);
            queue.push_back(then);
            queue.push_back(else_);
        }
        Expr::PatternMatch(expr, arms, _) => {
            queue.push_back(expr);
            for arm in arms {
                let arm_literal_expressions = arm.arm_pattern.get_expr_literals();
                queue.extend(arm_literal_expressions.iter().copied());
                queue.push_back(&*arm.arm_resolution_expr);
            }
        }
        Expr::Option(Some(expr), _) => queue.push_back(expr),
        Expr::Result(Ok(expr), _) => queue.push_back(expr),
        Expr::Result(Err(expr), _) => queue.push_back(expr),
        Expr::Call(_, expressions, _) => queue.extend(expressions.iter()),
        Expr::Unwrap(expr, _) => queue.push_back(expr),
        Expr::Get(expr, _, _) => queue.push_back(expr),
        Expr::And(expr1, expr2, _) => {
            queue.push_back(expr1);
            queue.push_back(expr2);
        },
        Expr::Literal(_, _) => {}
        Expr::Number(_, _, _) => {}
        Expr::Flags(_, _) => {}
        Expr::Identifier(_, _) => {}
        Expr::Boolean(_, _) => {}
        Expr::Option(None, _) => {}
        Expr::Throw(_, _) => {}
        Expr::Tag(_, _) => {}
    }
}

pub fn visit_children_mut_top_down<'a>(expr: &'a mut Expr, queue: &mut VecDeque<&'a mut Expr>) {
    match expr {
        Expr::Let(_, _, expr, _) => queue.push_front(&mut *expr),
        Expr::SelectField(expr, _, _) => queue.push_front(&mut *expr),
        Expr::SelectIndex(expr, _, _) => queue.push_front(&mut *expr),
        Expr::Sequence(exprs, _) => {
            for expr in exprs.iter_mut() {
                queue.push_front(expr);
            }
        }
        Expr::Record(exprs, _) => {
            for (_, expr) in exprs.iter_mut() {
                queue.push_front(&mut **expr);
            }
        }

        Expr::Tuple(exprs, _) => {
            for expr in exprs.iter_mut() {
                queue.push_front(expr);
            }
        }
        Expr::Concat(exprs, _) => {
            for expr in exprs.iter_mut() {
                queue.push_front(expr);
            }
        }
        Expr::Multiple(exprs, _) => {
            for expr in exprs.iter_mut() {
                queue.push_back(expr);
            }
        }
        Expr::Not(expr, _) => queue.push_front(&mut *expr),
        Expr::GreaterThan(lhs, rhs, _) => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs);
        }
        Expr::GreaterThanOrEqualTo(lhs, rhs, _) => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs);
        }
        Expr::LessThanOrEqualTo(lhs, rhs, _) => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs);
        }
        Expr::EqualTo(lhs, rhs, _) => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs);
        }
        Expr::LessThan(lhs, rhs, _) => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs);
        }
        Expr::Cond(cond, then, else_, _) => {
            queue.push_front(&mut *cond);
            queue.push_front(&mut *then);
            queue.push_front(&mut *else_);
        }
        Expr::PatternMatch(expr, arms, _) => {
            queue.push_front(&mut *expr);
            for arm in arms {
                let arm_literal_expressions = arm.arm_pattern.get_expr_literals_mut();
                queue.extend(arm_literal_expressions.into_iter().map(|x| x.as_mut()));
                queue.push_back(&mut *arm.arm_resolution_expr);
            }
        }
        Expr::Option(Some(expr), _) => queue.push_front(&mut *expr),
        Expr::Result(Ok(expr), _) => queue.push_front(&mut *expr),
        Expr::Result(Err(expr), _) => queue.push_front(&mut *expr),
        Expr::Call(_, expressions, _) => {
            for expr in expressions.iter_mut() {
                queue.push_front(expr);
            }
        }
        Expr::Unwrap(expr, _) => queue.push_front(&mut *expr),
        Expr::Get(expr, _, _) => queue.push_front(&mut *expr),
        Expr::And(expr1, expr2, _) => {
            queue.push_front(&mut *expr1);
            queue.push_front(&mut *expr2)
        },
        Expr::Literal(_, _) => {}
        Expr::Number(_, _, _) => {}
        Expr::Flags(_, _) => {}
        Expr::Identifier(_, _) => {}
        Expr::Boolean(_, _) => {}
        Expr::Option(None, _) => {}
        Expr::Throw(_, _) => {}
        Expr::Tag(_, _) => {}
    }
}
