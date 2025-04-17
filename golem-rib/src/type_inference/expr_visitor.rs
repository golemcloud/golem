use crate::call_type::{CallType, InstanceCreationType};
use crate::{Expr, InferredType};
use std::collections::VecDeque;
use std::ops::Deref;

// A structure that allows to visit expressions in a bottom-up or top-down order.
// All other functionalities are to be replaced with the usage of `ExprVisitor`
// https://github.com/golemcloud/golem/issues/1428
pub struct ExprVisitor<'a> {
    queue: VecDeque<&'a mut Expr>,
}

impl Default for ExprVisitor<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> ExprVisitor<'a> {
    pub fn new() -> Self {
        ExprVisitor {
            queue: VecDeque::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    // Enqueue expressions in a bottom-up order,
    // but in the natural order of rib program
    // Given
    //   `Expr::Block(Expr::Let(x, Expr::Num(1)), Expr::Call(func, x))`
    // Expr::Num(1)
    // Expr::Let(Variable(x), Expr::Num(1))
    // Expr::Identifier(x)
    // Expr::Call(func, Expr::Identifier(x))
    // Expr::Block(Expr::Let(x, Expr::Num(1)), Expr::Call(func, x))
    pub fn bottom_up(expr: &'a mut Expr) -> Self {
        let mut queue: VecDeque<&'a mut Expr> = VecDeque::new();

        enqueue_expr_bottom_up(expr, &mut queue);

        ExprVisitor { queue }
    }

    // Enqueue expressions in a top-down order,
    // while processing the expressions in the natural order within the block (Expr::Block).
    // Given
    //  `Expr::Block(Expr::Let(x, Expr::Num(1)), Expr::Call(func, x))`
    // Expr::Block(Expr::Let(x, Expr::Num(1)), Expr::Call(func, x))
    // Expr::Let(Variable(x), Expr::Num(1, U64))
    // Expr::Num(1, U64)
    // Expr::Call(func, Expr::Identifier(x))
    // Expr::Identifier(x)
    pub fn top_down(expr: &'a mut Expr) -> Self {
        let mut queue: VecDeque<&'a mut Expr> = VecDeque::new();

        enqueue_expr_top_down(expr, &mut queue);

        ExprVisitor { queue }
    }

    pub fn pop_front(&mut self) -> Option<&mut Expr> {
        self.queue.pop_front()
    }

    pub fn pop_back(&mut self) -> Option<&mut Expr> {
        self.queue.pop_back()
    }

    pub fn pop_all(&mut self) -> Vec<&mut Expr> {
        self.queue.drain(..).collect()
    }
}

fn enqueue_expr_top_down(expr: &mut Expr, queue: &mut VecDeque<&mut Expr>) {
    let mut stack: VecDeque<*mut Expr> = VecDeque::new();

    stack.push_back(expr);

    while let Some(current_ptr) = stack.pop_front() {
        queue.push_back(unsafe { &mut *current_ptr });

        let current = unsafe { &mut *current_ptr };

        match current {
            Expr::Let { expr, .. } => stack.push_front(&mut **expr),
            Expr::SelectField { expr, .. } => stack.push_front(&mut **expr),
            Expr::SelectIndex { expr, index, .. } => {
                stack.push_front(&mut **expr);
                stack.push_front(&mut **index);
            }
            Expr::Sequence { exprs, .. } => stack.extend(exprs.iter_mut().map(|x| x as *mut Expr)),
            Expr::Record { exprs, .. } => {
                stack.extend(exprs.iter_mut().map(|(_, expr)| &mut **expr as *mut Expr))
            }
            Expr::Tuple { exprs, .. } => stack.extend(exprs.iter_mut().map(|x| x as *mut Expr)),
            Expr::Concat { exprs, .. } => stack.extend(exprs.iter_mut().map(|x| x as *mut Expr)),
            Expr::ExprBlock { exprs, .. } => stack.extend(exprs.iter_mut().map(|x| x as *mut Expr)),
            Expr::Not { expr, .. } => stack.push_front(&mut **expr),
            Expr::Length { expr, .. } => stack.push_front(&mut **expr),
            Expr::GreaterThan { lhs, rhs, .. } => {
                stack.push_front(&mut **lhs);
                stack.push_front(&mut **rhs);
            }
            Expr::GreaterThanOrEqualTo { lhs, rhs, .. } => {
                stack.push_front(&mut **lhs);
                stack.push_front(&mut **rhs);
            }
            Expr::LessThanOrEqualTo { lhs, rhs, .. } => {
                stack.push_front(&mut **lhs);
                stack.push_front(&mut **rhs);
            }
            Expr::EqualTo { lhs, rhs, .. } => {
                stack.push_front(&mut **lhs);
                stack.push_front(&mut **rhs);
            }
            Expr::Plus { lhs, rhs, .. } => {
                stack.push_front(&mut **lhs);
                stack.push_front(&mut **rhs);
            }
            Expr::Minus { lhs, rhs, .. } => {
                stack.push_front(&mut **lhs);
                stack.push_front(&mut **rhs);
            }
            Expr::Divide { lhs, rhs, .. } => {
                stack.push_front(&mut **lhs);
                stack.push_front(&mut **rhs);
            }
            Expr::Multiply { lhs, rhs, .. } => {
                stack.push_front(&mut **lhs);
                stack.push_front(&mut **rhs);
            }
            Expr::LessThan { lhs, rhs, .. } => {
                stack.push_front(&mut **lhs);
                stack.push_front(&mut **rhs);
            }
            Expr::Cond { cond, lhs, rhs, .. } => {
                stack.push_front(&mut **cond);
                stack.push_front(&mut **lhs);
                stack.push_front(&mut **rhs);
            }
            Expr::PatternMatch {
                predicate,
                match_arms,
                ..
            } => {
                stack.push_front(&mut **predicate);
                for arm in match_arms {
                    let arm_literal_expressions = arm.arm_pattern.get_expr_literals_mut();

                    for x in arm_literal_expressions {
                        let x = x.as_mut();
                        stack.push_front(x);
                    }

                    stack.push_front(&mut *arm.arm_resolution_expr);
                }
            }

            Expr::Range { range, .. } => {
                for expr in range.get_exprs_mut() {
                    stack.push_front(&mut **expr);
                }
            }

            Expr::Option {
                expr: Some(expr), ..
            } => stack.push_front(&mut **expr),
            Expr::Result { expr: Ok(expr), .. } => stack.push_front(&mut **expr),
            Expr::Result {
                expr: Err(expr), ..
            } => stack.push_front(&mut **expr),
            Expr::Call {
                call_type,
                args,
                inferred_type,
                ..
            } => {
                let (exprs, worker) = internal::get_expressions_in_call_type_mut(call_type);
                if let Some(exprs) = exprs {
                    for x in exprs {
                        stack.push_front(x);
                    }
                }

                if let Some(worker) = worker {
                    stack.push_front(&mut **worker);
                }

                // The expr existing in the inferred type should be visited
                if let InferredType::Instance { instance_type } = inferred_type {
                    if let Some(worker_expr) = instance_type.worker_mut() {
                        stack.push_front(&mut **worker_expr);
                    }
                }

                for x in args {
                    stack.push_front(x);
                }
            }
            Expr::Unwrap { expr, .. } => stack.push_front(&mut **expr), // not yet needed
            Expr::And { lhs, rhs, .. } => {
                stack.push_front(&mut **lhs);
                stack.push_front(&mut **rhs)
            }

            Expr::Or { lhs, rhs, .. } => {
                stack.push_front(&mut **lhs);
                stack.push_front(&mut **rhs)
            }

            Expr::ListComprehension {
                iterable_expr,
                yield_expr,
                ..
            } => {
                stack.push_front(&mut **iterable_expr);
                stack.push_front(&mut **yield_expr);
            }

            Expr::ListReduce {
                iterable_expr,
                init_value_expr,
                yield_expr,
                ..
            } => {
                stack.push_front(&mut **iterable_expr);
                stack.push_front(&mut **init_value_expr);
                stack.push_front(&mut **yield_expr);
            }

            Expr::InvokeMethodLazy {
                lhs,
                args,
                inferred_type,
                ..
            } => {
                if let InferredType::Instance { instance_type } = inferred_type {
                    if let Some(worker_expr) = instance_type.worker_mut() {
                        stack.push_front(&mut **worker_expr);
                    }
                }

                stack.push_front(&mut **lhs);
                stack.extend(args.iter_mut().map(|x| x as *mut Expr));
            }

            Expr::GetTag { expr, .. } => {
                stack.push_front(&mut **expr);
            }

            Expr::Literal { .. } => {}
            Expr::Number { .. } => {}
            Expr::Flags { .. } => {}
            Expr::Identifier { .. } => {}
            Expr::Boolean { .. } => {}
            Expr::Option { expr: None, .. } => {}
            Expr::Throw { .. } => {}
        }
    }
}

fn enqueue_expr_bottom_up(expr: &mut Expr, queue: &mut VecDeque<&mut Expr>) {
    let mut stack: VecDeque<*mut Expr> = VecDeque::new();

    stack.push_back(expr);

    while let Some(current) = stack.pop_back() {
        queue.push_front(unsafe { &mut *current });

        let current = unsafe { &mut *current };

        match current {
            Expr::Let { expr, .. } => stack.push_back(&mut **expr),
            Expr::SelectField { expr, .. } => stack.push_back(&mut **expr),
            Expr::SelectIndex { expr, index, .. } => {
                stack.push_back(&mut **expr);
                stack.push_back(&mut **index);
            }
            Expr::Sequence { exprs, .. } => stack.extend(exprs.iter_mut().map(|x| x as *mut Expr)),
            Expr::Record { exprs, .. } => {
                stack.extend(exprs.iter_mut().map(|(_, expr)| &mut **expr as *mut Expr))
            }
            Expr::Tuple { exprs, .. } => stack.extend(exprs.iter_mut().map(|x| x as *mut Expr)),
            Expr::Concat { exprs, .. } => stack.extend(exprs.iter_mut().map(|x| x as *mut Expr)),
            Expr::ExprBlock { exprs, .. } => stack.extend(exprs.iter_mut().map(|x| x as *mut Expr)),
            Expr::Not { expr, .. } => stack.push_back(&mut **expr),
            Expr::Length { expr, .. } => stack.push_back(&mut **expr),
            Expr::GreaterThan { lhs, rhs, .. } => {
                stack.push_back(&mut **lhs);
                stack.push_back(&mut **rhs);
            }
            Expr::GreaterThanOrEqualTo { lhs, rhs, .. } => {
                stack.push_back(&mut **lhs);
                stack.push_back(&mut **rhs);
            }
            Expr::LessThanOrEqualTo { lhs, rhs, .. } => {
                stack.push_back(&mut **lhs);
                stack.push_back(&mut **rhs);
            }
            Expr::EqualTo { lhs, rhs, .. } => {
                stack.push_back(&mut **lhs);
                stack.push_back(&mut **rhs);
            }
            Expr::Plus { lhs, rhs, .. } => {
                stack.push_back(&mut **lhs);
                stack.push_back(&mut **rhs);
            }
            Expr::Minus { lhs, rhs, .. } => {
                stack.push_back(&mut **lhs);
                stack.push_back(&mut **rhs);
            }
            Expr::Divide { lhs, rhs, .. } => {
                stack.push_back(&mut **lhs);
                stack.push_back(&mut **rhs);
            }
            Expr::Multiply { lhs, rhs, .. } => {
                stack.push_back(&mut **lhs);
                stack.push_back(&mut **rhs);
            }
            Expr::LessThan { lhs, rhs, .. } => {
                stack.push_back(&mut **lhs);
                stack.push_back(&mut **rhs);
            }
            Expr::Cond { cond, lhs, rhs, .. } => {
                stack.push_back(&mut **cond);
                stack.push_back(&mut **lhs);
                stack.push_back(&mut **rhs);
            }
            Expr::PatternMatch {
                predicate,
                match_arms,
                ..
            } => {
                stack.push_back(&mut **predicate);
                for arm in match_arms {
                    let arm_literal_expressions = arm.arm_pattern.get_expr_literals_mut();
                    stack.extend(arm_literal_expressions.into_iter().map(|x| {
                        let x = x.as_mut();
                        x as *mut Expr
                    }));
                    stack.push_back(&mut *arm.arm_resolution_expr);
                }
            }

            Expr::Range { range, .. } => {
                for expr in range.get_exprs_mut() {
                    stack.push_back(&mut **expr);
                }
            }

            Expr::Option {
                expr: Some(expr), ..
            } => stack.push_back(&mut **expr),
            Expr::Result { expr: Ok(expr), .. } => stack.push_back(&mut **expr),
            Expr::Result {
                expr: Err(expr), ..
            } => stack.push_back(&mut **expr),
            Expr::Call {
                call_type,
                args,
                inferred_type,
                ..
            } => {
                let (exprs, worker) = internal::get_expressions_in_call_type_mut(call_type);
                if let Some(exprs) = exprs {
                    stack.extend(exprs.iter_mut().map(|x| x as *mut Expr))
                }

                if let Some(worker) = worker {
                    stack.push_back(&mut **worker);
                }

                // The expr existing in the inferred type should be visited
                if let InferredType::Instance { instance_type } = inferred_type {
                    if let Some(worker_expr) = instance_type.worker_mut() {
                        stack.push_back(&mut **worker_expr);
                    }
                }

                stack.extend(args.iter_mut().map(|x| x as *mut Expr))
            }
            Expr::Unwrap { expr, .. } => stack.push_back(&mut **expr),
            Expr::And { lhs, rhs, .. } => {
                stack.push_back(&mut **lhs);
                stack.push_back(&mut **rhs)
            }

            Expr::Or { lhs, rhs, .. } => {
                stack.push_back(&mut **lhs);
                stack.push_back(&mut **rhs)
            }

            Expr::ListComprehension {
                iterable_expr,
                yield_expr,
                ..
            } => {
                stack.push_back(&mut **iterable_expr);
                stack.push_back(&mut **yield_expr);
            }

            Expr::ListReduce {
                iterable_expr,
                init_value_expr,
                yield_expr,
                ..
            } => {
                stack.push_back(&mut **iterable_expr);
                stack.push_back(&mut **init_value_expr);
                stack.push_back(&mut **yield_expr);
            }

            Expr::InvokeMethodLazy {
                lhs,
                args,
                inferred_type,
                ..
            } => {
                if let InferredType::Instance { instance_type } = inferred_type {
                    if let Some(worker_expr) = instance_type.worker_mut() {
                        stack.push_back(&mut **worker_expr);
                    }
                }

                stack.push_back(&mut **lhs);
                stack.extend(args.iter_mut().map(|x| x as *mut Expr));
            }

            Expr::GetTag { expr, .. } => {
                stack.push_back(&mut **expr);
            }

            Expr::Literal { .. } => {}
            Expr::Number { .. } => {}
            Expr::Flags { .. } => {}
            Expr::Identifier { .. } => {}
            Expr::Boolean { .. } => {}
            Expr::Option { expr: None, .. } => {}
            Expr::Throw { .. } => {}
        }
    }
}

// This is to be replaced with the usage of `ExprVisitor`
// https://github.com/golemcloud/golem/issues/1428
pub fn visit_children_bottom_up_mut<'a>(expr: &'a mut Expr, queue: &mut VecDeque<&'a mut Expr>) {
    match expr {
        Expr::Let { expr, .. } => queue.push_back(&mut *expr),
        Expr::SelectField { expr, .. } => queue.push_back(&mut *expr),
        Expr::SelectIndex { expr, index, .. } => {
            queue.push_back(&mut *expr);
            queue.push_back(&mut *index);
        }
        Expr::Sequence { exprs, .. } => queue.extend(exprs.iter_mut()),
        Expr::Record { exprs, .. } => queue.extend(exprs.iter_mut().map(|(_, expr)| &mut **expr)),
        Expr::Tuple { exprs, .. } => queue.extend(exprs.iter_mut()),
        Expr::Concat { exprs, .. } => queue.extend(exprs.iter_mut()),
        Expr::ExprBlock { exprs, .. } => queue.extend(exprs.iter_mut()), // let x = 1, y = call(x);
        Expr::Not { expr, .. } => queue.push_back(&mut *expr),
        Expr::Length { expr, .. } => queue.push_back(&mut *expr),
        Expr::GreaterThan { lhs, rhs, .. } => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs);
        }
        Expr::GreaterThanOrEqualTo { lhs, rhs, .. } => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs);
        }
        Expr::LessThanOrEqualTo { lhs, rhs, .. } => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs);
        }
        Expr::EqualTo { lhs, rhs, .. } => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs);
        }
        Expr::Plus { lhs, rhs, .. } => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs);
        }
        Expr::Minus { lhs, rhs, .. } => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs);
        }
        Expr::Divide { lhs, rhs, .. } => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs);
        }
        Expr::Multiply { lhs, rhs, .. } => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs);
        }
        Expr::LessThan { lhs, rhs, .. } => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs);
        }
        Expr::Cond { cond, lhs, rhs, .. } => {
            queue.push_back(&mut *cond);
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs);
        }
        Expr::PatternMatch {
            predicate,
            match_arms,
            ..
        } => {
            queue.push_back(&mut *predicate);
            for arm in match_arms {
                let arm_literal_expressions = arm.arm_pattern.get_expr_literals_mut();
                queue.extend(arm_literal_expressions.into_iter().map(|x| x.as_mut()));
                queue.push_back(&mut *arm.arm_resolution_expr);
            }
        }

        Expr::Range { range, .. } => {
            for expr in range.get_exprs_mut() {
                queue.push_back(&mut *expr);
            }
        }

        Expr::Option {
            expr: Some(expr), ..
        } => queue.push_back(&mut *expr),
        Expr::Result { expr: Ok(expr), .. } => queue.push_back(&mut *expr),
        Expr::Result {
            expr: Err(expr), ..
        } => queue.push_back(&mut *expr),
        Expr::Call {
            call_type,
            args,
            inferred_type,
            ..
        } => {
            let (exprs, worker) = internal::get_expressions_in_call_type_mut(call_type);
            if let Some(exprs) = exprs {
                queue.extend(exprs.iter_mut())
            }

            if let Some(worker) = worker {
                queue.push_back(worker);
            }

            // The expr existing in the inferred type should be visited
            if let InferredType::Instance { instance_type } = inferred_type {
                if let Some(worker_expr) = instance_type.worker_mut() {
                    queue.push_back(worker_expr);
                }
            }

            queue.extend(args.iter_mut())
        }
        Expr::Unwrap { expr, .. } => queue.push_back(&mut *expr), // not yet needed
        Expr::And { lhs, rhs, .. } => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs)
        }

        Expr::Or { lhs, rhs, .. } => {
            queue.push_back(&mut *lhs);
            queue.push_back(&mut *rhs)
        }

        Expr::ListComprehension {
            iterable_expr,
            yield_expr,
            ..
        } => {
            queue.push_back(&mut *iterable_expr);
            queue.push_back(&mut *yield_expr);
        }

        Expr::ListReduce {
            iterable_expr,
            init_value_expr,
            yield_expr,
            ..
        } => {
            queue.push_back(iterable_expr);
            queue.push_back(init_value_expr);
            queue.push_back(yield_expr);
        }

        Expr::InvokeMethodLazy {
            lhs,
            args,
            inferred_type,
            ..
        } => {
            if let InferredType::Instance { instance_type } = inferred_type {
                if let Some(worker_expr) = instance_type.worker_mut() {
                    queue.push_back(worker_expr);
                }
            }

            queue.push_back(lhs);
            queue.extend(args.iter_mut());
        }

        Expr::GetTag { expr, .. } => {
            queue.push_back(&mut *expr);
        }

        Expr::Literal { .. } => {}
        Expr::Number { .. } => {}
        Expr::Flags { .. } => {}
        Expr::Identifier { .. } => {}
        Expr::Boolean { .. } => {}
        Expr::Option { expr: None, .. } => {}
        Expr::Throw { .. } => {}
    }
}

// This is to be replaced with the usage of `ExprVisitor`
// https://github.com/golemcloud/golem/issues/1428
pub fn visit_children_bottom_up<'a>(expr: &'a Expr, queue: &mut VecDeque<&'a Expr>) {
    match expr {
        Expr::Let { expr, .. } => queue.push_back(expr),
        Expr::SelectField { expr, .. } => queue.push_back(expr),
        Expr::SelectIndex { expr, index, .. } => {
            queue.push_back(expr);
            queue.push_back(index);
        }
        Expr::Sequence { exprs, .. } => queue.extend(exprs.iter()),
        Expr::Record { exprs, .. } => queue.extend(exprs.iter().map(|(_, expr)| expr.deref())),
        Expr::Tuple { exprs, .. } => queue.extend(exprs.iter()),
        Expr::Concat { exprs, .. } => queue.extend(exprs.iter()),
        Expr::ExprBlock { exprs, .. } => queue.extend(exprs.iter()),
        Expr::Length { expr, .. } => queue.push_back(expr),

        Expr::Not { expr, .. } => queue.push_back(expr),
        Expr::GreaterThan { lhs, rhs, .. } => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::GreaterThanOrEqualTo { lhs, rhs, .. } => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::LessThanOrEqualTo { lhs, rhs, .. } => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::EqualTo { lhs, rhs, .. } => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::Plus { lhs, rhs, .. } => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::Minus { lhs, rhs, .. } => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::Divide { lhs, rhs, .. } => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::Multiply { lhs, rhs, .. } => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::LessThan { lhs, rhs, .. } => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::Cond { cond, lhs, rhs, .. } => {
            queue.push_back(cond);
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::PatternMatch {
            predicate,
            match_arms,
            ..
        } => {
            queue.push_back(predicate);
            for arm in match_arms {
                let arm_literal_expressions = arm.arm_pattern.get_expr_literals();
                queue.extend(arm_literal_expressions.iter().copied());
                queue.push_back(&*arm.arm_resolution_expr);
            }
        }
        Expr::Option {
            expr: Some(expr), ..
        } => queue.push_back(expr),
        Expr::Result { expr: Ok(expr), .. } => queue.push_back(expr),
        Expr::Result {
            expr: Err(expr), ..
        } => queue.push_back(expr),
        Expr::Call {
            call_type,
            args,
            inferred_type,
            ..
        } => {
            if let CallType::Function {
                function_name,
                worker,
            } = call_type
            {
                if let Some(params) = function_name.function.raw_resource_params() {
                    queue.extend(params.iter())
                }

                // Worker in InstanceType
                if let InferredType::Instance { instance_type } = inferred_type {
                    if let Some(worker_expr) = instance_type.worker() {
                        queue.push_back(worker_expr);
                    }
                }

                // Worker in Call Expression
                if let Some(worker) = worker {
                    queue.push_back(worker);
                }
            }

            if let CallType::InstanceCreation(instance_creation) = call_type {
                if let InferredType::Instance { instance_type } = inferred_type {
                    if let Some(worker_expr) = instance_type.worker() {
                        queue.push_back(worker_expr);
                    }
                }

                match instance_creation {
                    InstanceCreationType::Worker { worker_name, .. } => {
                        if let Some(worker_name) = worker_name {
                            queue.push_back(worker_name);
                        }
                    }

                    InstanceCreationType::Resource { worker_name, .. } => {
                        if let Some(worker_name) = worker_name {
                            queue.push_back(worker_name);
                        }
                    }
                }
            }

            queue.extend(args.iter())
        }
        Expr::Unwrap { expr, .. } => queue.push_back(expr),
        Expr::And { lhs, rhs, .. } => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::Or { lhs, rhs, .. } => {
            queue.push_back(lhs);
            queue.push_back(rhs);
        }
        Expr::ListComprehension {
            iterable_expr,
            yield_expr,
            ..
        } => {
            queue.push_back(iterable_expr);
            queue.push_back(yield_expr)
        }
        Expr::ListReduce {
            iterable_expr,
            init_value_expr,
            yield_expr,
            ..
        } => {
            queue.push_back(iterable_expr);
            queue.push_back(init_value_expr);
            queue.push_back(yield_expr);
        }
        Expr::GetTag { expr, .. } => {
            queue.push_back(expr);
        }

        Expr::Range { range, .. } => {
            let exprs = range.get_exprs();

            queue.extend(exprs.iter());
        }

        Expr::InvokeMethodLazy {
            lhs,
            args,
            inferred_type,
            ..
        } => {
            if let InferredType::Instance { instance_type } = inferred_type {
                if let Some(worker_expr) = instance_type.worker() {
                    queue.push_back(worker_expr);
                }
            }

            queue.push_back(lhs);
            queue.extend(args.iter());
        }

        Expr::Literal { .. } => {}
        Expr::Number { .. } => {}
        Expr::Flags { .. } => {}
        Expr::Identifier { .. } => {}
        Expr::Boolean { .. } => {}
        Expr::Option { expr: None, .. } => {}
        Expr::Throw { .. } => {}
    }
}

// This is to be replaced with the usage of `ExprVisitor`
// https://github.com/golemcloud/golem/issues/1428
pub fn visit_children_mut_top_down<'a>(expr: &'a mut Expr, queue: &mut VecDeque<&'a mut Expr>) {
    match expr {
        Expr::Let { expr, .. } => queue.push_front(&mut *expr),
        Expr::SelectField { expr, .. } => queue.push_front(&mut *expr),
        Expr::SelectIndex { expr, index, .. } => {
            queue.push_front(&mut *expr);
            queue.push_front(&mut *index);
        }
        Expr::Sequence { exprs, .. } => {
            for expr in exprs.iter_mut() {
                queue.push_front(expr);
            }
        }

        Expr::Length { expr, .. } => queue.push_front(&mut *expr),

        Expr::Record { exprs, .. } => {
            for (_, expr) in exprs.iter_mut() {
                queue.push_front(&mut **expr);
            }
        }

        Expr::Tuple { exprs, .. } => {
            for expr in exprs.iter_mut() {
                queue.push_front(expr);
            }
        }
        Expr::Concat { exprs, .. } => {
            for expr in exprs.iter_mut() {
                queue.push_front(expr);
            }
        }

        Expr::Range { range, .. } => {
            for expr in range.get_exprs_mut() {
                queue.push_front(&mut *expr);
            }
        }

        Expr::ExprBlock { exprs, .. } => {
            for expr in exprs.iter_mut() {
                queue.push_back(expr);
            }
        }
        Expr::Not { expr, .. } => queue.push_front(&mut *expr),
        Expr::GreaterThan { lhs, rhs, .. } => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs);
        }
        Expr::GreaterThanOrEqualTo { lhs, rhs, .. } => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs);
        }
        Expr::LessThanOrEqualTo { lhs, rhs, .. } => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs);
        }
        Expr::EqualTo { lhs, rhs, .. } => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs);
        }
        Expr::Plus { lhs, rhs, .. } => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs);
        }
        Expr::Minus { lhs, rhs, .. } => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs);
        }
        Expr::Divide { lhs, rhs, .. } => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs);
        }
        Expr::Multiply { lhs, rhs, .. } => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs);
        }
        Expr::LessThan { lhs, rhs, .. } => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs);
        }
        Expr::Cond { cond, lhs, rhs, .. } => {
            queue.push_front(&mut *cond);
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs);
        }
        Expr::And { lhs, rhs, .. } => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs)
        }
        Expr::Or { lhs, rhs, .. } => {
            queue.push_front(&mut *lhs);
            queue.push_front(&mut *rhs)
        }
        Expr::PatternMatch {
            predicate,
            match_arms,
            ..
        } => {
            queue.push_front(&mut *predicate);
            for arm in match_arms {
                let arm_literal_expressions = arm.arm_pattern.get_expr_literals_mut();
                queue.extend(arm_literal_expressions.into_iter().map(|x| x.as_mut()));
                queue.push_back(&mut *arm.arm_resolution_expr);
            }
        }
        Expr::Option {
            expr: Some(expr), ..
        } => queue.push_front(&mut *expr),
        Expr::Result { expr: Ok(expr), .. } => queue.push_front(&mut *expr),
        Expr::Result {
            expr: Err(expr), ..
        } => queue.push_front(&mut *expr),
        Expr::Call {
            call_type,
            args,
            inferred_type,
            ..
        } => {
            let (exprs, worker) = internal::get_expressions_in_call_type_mut(call_type);

            if let Some(exprs) = exprs {
                for expr in exprs.iter_mut() {
                    queue.push_front(expr);
                }
            }

            if let Some(worker) = worker {
                queue.push_front(worker);
            }

            // The expr existing in the inferred type should be visited
            if let InferredType::Instance { instance_type } = inferred_type {
                if let Some(worker_expr) = instance_type.worker_mut() {
                    queue.push_back(worker_expr);
                }
            }

            for expr in args.iter_mut() {
                queue.push_front(expr);
            }
        }
        Expr::GetTag { expr, .. } => {
            queue.push_front(&mut *expr);
        }
        Expr::ListComprehension {
            iterable_expr,
            yield_expr,
            ..
        } => {
            queue.push_front(iterable_expr);
            queue.push_front(yield_expr)
        }
        Expr::ListReduce {
            iterable_expr,
            init_value_expr,
            yield_expr,
            ..
        } => {
            queue.push_front(iterable_expr);
            queue.push_front(init_value_expr);
            queue.push_front(yield_expr);
        }

        Expr::InvokeMethodLazy {
            lhs,
            args,
            inferred_type,
            ..
        } => {
            if let InferredType::Instance { instance_type } = inferred_type {
                if let Some(worker_expr) = instance_type.worker_mut() {
                    queue.push_front(worker_expr);
                }
            }
            queue.push_front(lhs);
            for arg in args.iter_mut() {
                queue.push_front(arg);
            }
        }

        Expr::Unwrap { expr, .. } => queue.push_front(&mut *expr),
        Expr::Literal { .. } => {}
        Expr::Number { .. } => {}
        Expr::Flags { .. } => {}
        Expr::Identifier { .. } => {}
        Expr::Boolean { .. } => {}
        Expr::Option { expr: None, .. } => {}
        Expr::Throw { .. } => {}
    }
}

mod internal {
    use crate::call_type::{CallType, InstanceCreationType};
    use crate::Expr;

    // (args, worker in calls, worker in inferred type)
    pub(crate) fn get_expressions_in_call_type_mut(
        call_type: &mut CallType,
    ) -> (Option<&mut Vec<Expr>>, Option<&mut Box<Expr>>) {
        match call_type {
            CallType::Function {
                function_name,
                worker,
            } => (
                function_name.function.raw_resource_params_mut(),
                worker.as_mut(),
            ),

            CallType::InstanceCreation(instance_creation) => match instance_creation {
                InstanceCreationType::Worker { worker_name, .. } => (None, worker_name.as_mut()),

                InstanceCreationType::Resource { worker_name, .. } => (None, worker_name.as_mut()),
            },

            CallType::VariantConstructor(_) => (None, None),
            CallType::EnumConstructor(_) => (None, None),
        }
    }
}
