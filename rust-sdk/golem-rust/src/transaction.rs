// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::fmt::{Debug, Display, Formatter};
use std::rc::Rc;

use crate::bindings::golem::api::host::{get_oplog_index, set_oplog_index, OplogIndex};
use crate::mark_atomic_operation;

/// Represents an atomic operation of the transaction which has a rollback action.
///
/// Implement this trait and use it within a `transaction` block.
/// Operations can also be constructed from closures using `operation`.
pub trait Operation: Clone {
    type In: Clone;
    type Out;
    type Err;

    /// Executes the operation which may fail with a domain error
    fn execute(&self, input: Self::In) -> Result<Self::Out, Self::Err>;

    /// Executes a compensation action for the operation. This version has no access to the result
    /// of the `execute` function, so it can be called in case of compensating advanced, non-domain level errors.
    ///
    /// If the operation is only used in `FallibleTransaction`s, this method can be no-op and the actual, result
    /// dependent compensation can be implemented in `compensate_with_result`.
    fn compensate(&self, input: Self::In) -> Result<(), Self::Err>;

    /// Executes a compensation action for the operation which ended up with the given result.
    fn compensate_with_result(
        &self,
        input: Self::In,
        _result: Result<Self::Out, Self::Err>,
    ) -> Result<(), Self::Err> {
        self.compensate(input)
    }
}

/// Constructs an `Operation` from two closures: one for executing the operation,
/// and one for rolling it back. The rollback operation only sees the input of the operation,
/// not the operation's result.
///
/// This operation can run the compensation in both fallible and infallible transactions.
pub fn operation<In: Clone, Out, Err>(
    execute_fn: impl Fn(In) -> Result<Out, Err> + 'static,
    compensate_fn: impl Fn(In) -> Result<(), Err> + 'static,
) -> impl Operation<In = In, Out = Out, Err = Err> {
    FnOperation {
        execute_fn: Rc::new(execute_fn),
        compensate_fn: Rc::new(compensate_fn),
    }
}

/// Constructs an `Operation` from two closures: one for executing the operation,
/// and one for rolling it back where the rollback operation can see the operation's result.
///
/// This operation can not be used with `infallible_transaction_with_strong_rollback_guarantees`.
pub fn operation_with_result<In: Clone, Out, Err>(
    execute_fn: impl Fn(In) -> Result<Out, Err> + 'static,
    compensate_fn: impl Fn(In, Result<Out, Err>) -> Result<(), Err> + 'static,
) -> impl Operation<In = In, Out = Out, Err = Err> {
    FnOperationWithResult {
        execute_fn: Rc::new(execute_fn),
        compensate_fn: Rc::new(compensate_fn),
    }
}

struct FnOperation<In, Out, Err> {
    execute_fn: Rc<dyn Fn(In) -> Result<Out, Err>>,
    compensate_fn: Rc<dyn Fn(In) -> Result<(), Err>>,
}

impl<In, Out, Err> Clone for FnOperation<In, Out, Err> {
    fn clone(&self) -> Self {
        Self {
            execute_fn: self.execute_fn.clone(),
            compensate_fn: self.compensate_fn.clone(),
        }
    }
}

impl<In: Clone, Out, Err> Operation for FnOperation<In, Out, Err> {
    type In = In;
    type Out = Out;
    type Err = Err;

    fn execute(&self, input: In) -> Result<Out, Err> {
        (self.execute_fn)(input)
    }

    fn compensate(&self, input: In) -> Result<(), Err> {
        (self.compensate_fn)(input)
    }
}

#[allow(clippy::type_complexity)]
struct FnOperationWithResult<In, Out, Err> {
    execute_fn: Rc<dyn Fn(In) -> Result<Out, Err>>,
    compensate_fn: Rc<dyn Fn(In, Result<Out, Err>) -> Result<(), Err>>,
}

impl<In, Out, Err> Clone for FnOperationWithResult<In, Out, Err> {
    fn clone(&self) -> Self {
        Self {
            execute_fn: self.execute_fn.clone(),
            compensate_fn: self.compensate_fn.clone(),
        }
    }
}

impl<In: Clone, Out, Err> Operation for FnOperationWithResult<In, Out, Err> {
    type In = In;
    type Out = Out;
    type Err = Err;

    fn execute(&self, input: In) -> Result<Out, Err> {
        (self.execute_fn)(input)
    }

    fn compensate(&self, _input: Self::In) -> Result<(), Self::Err> {
        Ok(())
    }

    fn compensate_with_result(&self, input: In, result: Result<Out, Err>) -> Result<(), Err> {
        (self.compensate_fn)(input, result)
    }
}

/// The result of a transaction execution.
pub type TransactionResult<Out, Err> = Result<Out, TransactionFailure<Err>>;

/// The result of a transaction execution that failed.
#[derive(Debug)]
pub enum TransactionFailure<Err> {
    /// One of the operations failed with an error, and the transaction was fully rolled back.
    FailedAndRolledBackCompletely(Err),
    /// One of the operations failed with an error, and the transaction was partially rolled back
    /// because the compensation action of one of the operations also failed.
    FailedAndRolledBackPartially {
        failure: Err,
        compensation_failure: Err,
    },
}

impl<Err: Display> Display for TransactionFailure<Err> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TransactionFailure::FailedAndRolledBackCompletely(err) => {
                write!(f, "Transaction failed with {err} and rolled back completely.")
            }
            TransactionFailure::FailedAndRolledBackPartially {
                failure,
                compensation_failure,
            } => write!(
                f,
                "Transaction failed with {failure} and rolled back partially; compensation failed with: {compensation_failure}."
            ),
        }
    }
}

/// Fallible transaction execution. If any operation fails, all the already executed
/// operation's compensation actions are executed in reverse order and the transaction
/// returns with a failure.
pub fn fallible_transaction<Out, Err: Clone + 'static>(
    f: impl FnOnce(&mut FallibleTransaction<Err>) -> Result<Out, Err>,
) -> TransactionResult<Out, Err> {
    let mut transaction = FallibleTransaction::new();
    match f(&mut transaction) {
        Ok(output) => Ok(output),
        Err(error) => Err(transaction.on_fail(error)),
    }
}

/// Retry the transaction in case of failure. If any operation returns with a failure, all
/// the already executed operation's compensation actions are executed in reverse order
/// and the transaction gets retried, using Golem's active retry policy.
pub fn infallible_transaction<Out>(f: impl FnOnce(&mut InfallibleTransaction) -> Out) -> Out {
    let oplog_index = get_oplog_index();
    let _atomic_region = mark_atomic_operation();
    let mut transaction = InfallibleTransaction::new(oplog_index);
    f(&mut transaction)
}

/// Same as `infallible_transaction`, but with strong rollback guarantees. The compensation actions
/// are guaranteed to be always executed before the transaction gets retried, even if it
/// fails due to a panic or an external executor failure.
pub fn infallible_transaction_with_strong_rollback_guarantees<Out>(
    _f: impl FnOnce(&mut InfallibleTransaction) -> Out,
) -> Out {
    unimplemented!()
}

/// A generic interface for defining transactions, where the transaction mode is
/// determined by the function's parameter (it can be `FallibleTransaction` or `InfallibleTransaction`).
///
/// This makes switching between different transaction guarantees easier, but is more constrained
/// than using the specific transaction functions where for retried transactions errors does
/// not have to be handled.
pub fn transaction<Out, Err, F, T>(f: F) -> TransactionResult<Out, Err>
where
    T: Transaction<Err>,
    F: FnOnce(&mut T) -> Result<Out, Err>,
{
    T::run(f)
}

/// Helper trait for coupling compensation action and the result of the operation.
trait CompensationAction<Err> {
    fn execute(&self) -> Result<(), Err>;
}

/// Helper struct for coupling compensation action and the result of the operation.
#[allow(clippy::type_complexity)]
struct CompensationActionCell<Out, Err> {
    action: Box<dyn Fn(Result<Out, Err>) -> Result<(), Err>>,
    result: Option<Result<Out, Err>>,
}

impl<Out: Clone, Err: Clone> CompensationAction<Err> for CompensationActionCell<Out, Err> {
    fn execute(&self) -> Result<(), Err> {
        let action = &*self.action;
        action(
            self.result
                .clone()
                .expect("Compensation action executed without a result"),
        )
    }
}

/// FallibleTransaction is a sequence of operations that are executed in a way that if any of the
/// operations fails all the already performed operation's compensation actions got executed in
/// reverse order.
///
/// In case of fatal errors (panic) and external executor failures it does not perform the
/// compensation actions and the whole transaction gets retried.
pub struct FallibleTransaction<Err> {
    compensations: Vec<Box<dyn CompensationAction<Err>>>,
}

impl<Err: Clone + 'static> FallibleTransaction<Err> {
    fn new() -> Self {
        Self {
            compensations: Vec::new(),
        }
    }

    pub fn execute<OpIn: Clone + 'static, OpOut: Clone + 'static>(
        &mut self,
        operation: impl Operation<In = OpIn, Out = OpOut, Err = Err> + 'static,
        input: OpIn,
    ) -> Result<OpOut, Err> {
        let cloned_op = operation.clone();
        let cloned_in = input.clone();
        let mut cell = CompensationActionCell {
            action: Box::new(move |result| {
                cloned_op.compensate_with_result(cloned_in.clone(), result)
            }),
            result: None,
        };
        let result = operation.execute(input);
        cell.result = Some(result.clone());
        self.compensations.push(Box::new(cell));
        result
    }

    fn on_fail(&mut self, failure: Err) -> TransactionFailure<Err> {
        for compensation_action in self.compensations.drain(..).rev() {
            if let Err(compensation_failure) = compensation_action.execute() {
                return TransactionFailure::FailedAndRolledBackPartially {
                    failure,
                    compensation_failure,
                };
            }
        }
        TransactionFailure::FailedAndRolledBackCompletely(failure)
    }
}

/// RetriedTransaction is a sequence of operations that are executed in a way that if any of the
/// operations or the underlying Golem executor fails, the whole transaction is going to
/// be retried.
///
/// In addition to that, **user level failures** (represented by the `Result::Err` value
/// of an operation) lead to performing the compensation actions of each already performed operation
/// in reverse order.
///
/// Fatal errors (panic) and external executor failures are currently cannot perform the
/// rollback actions.
pub struct InfallibleTransaction {
    begin_oplog_index: OplogIndex,
    compensations: Vec<Box<dyn CompensationAction<()>>>,
}

impl InfallibleTransaction {
    fn new(begin_oplog_index: OplogIndex) -> Self {
        Self {
            begin_oplog_index,
            compensations: Vec::new(),
        }
    }

    pub fn execute<
        OpIn: Clone + 'static,
        OpOut: Clone + 'static,
        OpErr: Debug + Clone + 'static,
    >(
        &mut self,
        operation: impl Operation<In = OpIn, Out = OpOut, Err = OpErr> + 'static,
        input: OpIn,
    ) -> OpOut {
        let cloned_op = operation.clone();
        let cloned_in = input.clone();
        let mut cell = CompensationActionCell {
            action: Box::new(move |result| {
                cloned_op
                    .compensate_with_result(cloned_in.clone(), result)
                    .expect("Compensation action failed");
                Ok(())
            }),
            result: None,
        };
        let result = operation.execute(input);
        cell.result = Some(result.clone());
        match result {
            Ok(output) => output,
            Err(_) => {
                self.retry();
                unreachable!()
            }
        }
    }

    /// Stop executing the transaction and retry from the beginning, after executing the compensation actions
    pub fn retry(&mut self) {
        for compensation_action in self.compensations.drain(..).rev() {
            let _ = compensation_action.execute();
        }
        set_oplog_index(self.begin_oplog_index);
    }
}

/// A unified interface for the different types of transactions. Using it can makes the code
/// easier to switch between different transactional guarantees but is more constrained in
/// terms of error types.
pub trait Transaction<Err> {
    fn execute<OpIn: Clone + 'static, OpOut: Clone + 'static>(
        &mut self,
        operation: impl Operation<In = OpIn, Out = OpOut, Err = Err> + 'static,
        input: OpIn,
    ) -> Result<OpOut, Err>;

    fn fail(&mut self, error: Err) -> Result<(), Err>;

    fn run<Out>(f: impl FnOnce(&mut Self) -> Result<Out, Err>) -> TransactionResult<Out, Err>;
}

impl<Err: Clone + 'static> Transaction<Err> for FallibleTransaction<Err> {
    fn execute<OpIn: Clone + 'static, OpOut: Clone + 'static>(
        &mut self,
        operation: impl Operation<In = OpIn, Out = OpOut, Err = Err> + 'static,
        input: OpIn,
    ) -> Result<OpOut, Err> {
        FallibleTransaction::execute(self, operation, input)
    }

    fn fail(&mut self, error: Err) -> Result<(), Err> {
        Err(error)
    }

    fn run<Out>(f: impl FnOnce(&mut Self) -> Result<Out, Err>) -> TransactionResult<Out, Err> {
        fallible_transaction(f)
    }
}

impl<Err: Debug + Clone + 'static> Transaction<Err> for InfallibleTransaction {
    fn execute<OpIn: Clone + 'static, OpOut: Clone + 'static>(
        &mut self,
        operation: impl Operation<In = OpIn, Out = OpOut, Err = Err> + 'static,
        input: OpIn,
    ) -> Result<OpOut, Err> {
        Ok(InfallibleTransaction::execute(self, operation, input))
    }

    fn fail(&mut self, error: Err) -> Result<(), Err> {
        InfallibleTransaction::retry(self);
        Err(error)
    }

    fn run<Out>(f: impl FnOnce(&mut Self) -> Result<Out, Err>) -> TransactionResult<Out, Err> {
        Ok(infallible_transaction(|tx| f(tx).unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::{fallible_transaction, infallible_transaction, operation, operation_with_result};

    // Not a real test, just verifying that the code compiles
    #[test]
    #[ignore]
    fn tx_test_1() {
        let log = Rc::new(RefCell::new(Vec::new()));

        let log1 = log.clone();
        let log2 = log.clone();
        let log3 = log.clone();
        let log4 = log.clone();

        let op1 = operation(
            move |input: String| {
                log1.borrow_mut().push(format!("op1 execute {input}"));
                Ok(())
            },
            move |input: String| {
                log2.borrow_mut().push(format!("op1 rollback {input}"));
                Ok(())
            },
        );

        let op2 = operation(
            move |_: ()| {
                log3.clone().borrow_mut().push("op2 execute".to_string());
                Err::<(), &str>("op2 error")
            },
            move |_: ()| {
                log4.clone().borrow_mut().push("op2 rollback".to_string());
                Ok(())
            },
        );

        let result = fallible_transaction(|tx| {
            println!("First we execute op1");
            tx.execute(op1, "hello".to_string())?;
            println!("Then execute op2");
            tx.execute(op2, ())?;
            println!("Finally compute a result");
            Ok(11)
        });

        println!("{log:?}");
        println!("{result:?}");
    }

    // Not a real test, just verifying that the code compiles
    #[test]
    #[ignore]
    fn tx_test_2() {
        let log = Rc::new(RefCell::new(Vec::new()));

        let log1 = log.clone();
        let log2 = log.clone();
        let log3 = log.clone();
        let log4 = log.clone();

        let op1 = operation(
            move |input: String| {
                log1.borrow_mut().push(format!("op1 execute {input}"));
                Ok::<(), ()>(())
            },
            move |input: String| {
                log2.borrow_mut().push(format!("op1 rollback {input}"));
                Ok(())
            },
        );

        let op2 = operation_with_result(
            move |_: ()| {
                log3.clone().borrow_mut().push("op2 execute".to_string());
                Err::<(), &str>("op2 error")
            },
            move |_: (), r| {
                log4.clone()
                    .borrow_mut()
                    .push(format!("op2 rollback {r:?}"));
                Ok(())
            },
        );

        let result = infallible_transaction(|tx| {
            println!("First we execute op1");
            tx.execute(op1, "hello".to_string());
            println!("Then execute op2");
            tx.execute(op2, ());
            println!("Finally compute a result");
            11
        });

        println!("{log:?}");
        println!("{result:?}");
    }
}

#[cfg(test)]
#[cfg(feature = "macro")]
mod macro_tests {
    use golem_rust_macro::golem_operation;

    use crate::{fallible_transaction, infallible_transaction};

    mod golem_rust {
        pub use crate::*;
    }

    #[golem_operation(compensation=test_compensation)]
    fn test_operation(input1: u64, input2: f32) -> Result<bool, String> {
        println!("Op input: {input1}, {input2}");
        Ok(true)
    }

    fn test_compensation(input1: u64, input2: f32) -> Result<(), String> {
        println!("Compensation input: {input1}, {input2}");
        Ok(())
    }

    #[golem_operation(compensation_with_result=test_compensation_2)]
    fn test_operation_2(input1: u64, input2: f32) -> Result<bool, String> {
        println!("Op input: {input1}, {input2}");
        Ok(true)
    }

    fn test_compensation_2(
        input1: u64,
        input2: f32,
        result: Result<bool, String>,
    ) -> Result<(), String> {
        println!("Compensation input: {input1}, {input2} for operation {result:?}");
        Ok(())
    }

    // Not a real test, just verifying that the code compiles
    #[test]
    #[ignore]
    fn tx_test_1() {
        let result = fallible_transaction(|tx| {
            println!("Executing the annotated function as an operation directly");
            tx.test_operation(1, 0.1)?;
            tx.test_operation_2(1, 0.1)?;

            Ok(11)
        });

        println!("{result:?}");
    }

    // Not a real test, just verifying that the code compiles
    #[test]
    #[ignore]
    fn tx_test_2() {
        let result = infallible_transaction(|tx| {
            println!("Executing the annotated function as an operation directly");
            let _ = tx.test_operation(1, 0.1);
            11
        });

        println!("{result:?}");
    }
}
