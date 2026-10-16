// Copyright 2024-2026 Golem Cloud
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

mod compfn;

use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

pub use compfn::*;

/// A boxed local future. Used as the return type for transaction closures
/// to allow the returned future to borrow from the transaction reference.
///
/// Use [`boxed`] to construct this from an async block.
pub type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Wraps an async block into a [`LocalBoxFuture`]. Use this when passing
/// closures to [`fallible_transaction`] or [`infallible_transaction`]:
///
/// ```ignore
/// fallible_transaction(|tx| boxed(async move {
///     tx.execute(op1, input).await?;
///     Ok(result)
/// })).await;
/// ```
#[inline]
pub fn boxed<'a, T>(fut: impl Future<Output = T> + 'a) -> LocalBoxFuture<'a, T> {
    Box::pin(fut)
}

/// Represents a compensable operation of the transaction.
///
/// Implement this trait and use it within a `transaction` block.
/// Operations can also be constructed from closures using `operation` or `sync_operation`.
#[allow(async_fn_in_trait)]
pub trait Operation: Clone {
    type In: Clone;
    type Out: Clone;
    type Err: Clone;

    /// Executes the operation which may fail with a domain error
    async fn execute(&self, input: Self::In) -> Result<Self::Out, Self::Err>;

    /// Executes a compensation action for the operation.
    async fn compensate(&self, input: Self::In, result: Self::Out) -> Result<(), Self::Err>;
}

/// Constructs an `Operation` from two async closures: one for executing the operation,
/// and one for rolling it back. The rollback operation always sees the input and
/// the output of the operation.
///
/// This operation can run the compensation in both fallible and infallible transactions.
#[allow(clippy::type_complexity)]
pub fn operation<In: Clone, Out: Clone, Err: Clone>(
    execute_fn: impl Fn(In) -> Pin<Box<dyn Future<Output = Result<Out, Err>>>> + 'static,
    compensate_fn: impl Fn(In, Out) -> Pin<Box<dyn Future<Output = Result<(), Err>>>> + 'static,
) -> impl Operation<In = In, Out = Out, Err = Err> {
    FnOperation {
        execute_fn: Rc::new(execute_fn),
        compensate_fn: Rc::new(compensate_fn),
    }
}

/// Constructs an `Operation` from two synchronous closures: one for executing the operation,
/// and one for rolling it back. This is a convenience wrapper around `operation` for
/// cases where the execute and compensate logic is synchronous.
pub fn sync_operation<In: Clone + 'static, Out: Clone + 'static, Err: Clone + 'static>(
    execute_fn: impl Fn(In) -> Result<Out, Err> + 'static,
    compensate_fn: impl Fn(In, Out) -> Result<(), Err> + 'static,
) -> impl Operation<In = In, Out = Out, Err = Err> {
    let execute_fn = Rc::new(execute_fn);
    let compensate_fn = Rc::new(compensate_fn);
    operation(
        move |input| {
            let f = execute_fn.clone();
            Box::pin(async move { f(input) })
        },
        move |input, output| {
            let f = compensate_fn.clone();
            Box::pin(async move { f(input, output) })
        },
    )
}

#[allow(clippy::type_complexity)]
struct FnOperation<In, Out, Err> {
    execute_fn: Rc<dyn Fn(In) -> Pin<Box<dyn Future<Output = Result<Out, Err>>>>>,
    compensate_fn: Rc<dyn Fn(In, Out) -> Pin<Box<dyn Future<Output = Result<(), Err>>>>>,
}

impl<In, Out, Err> Clone for FnOperation<In, Out, Err> {
    fn clone(&self) -> Self {
        Self {
            execute_fn: self.execute_fn.clone(),
            compensate_fn: self.compensate_fn.clone(),
        }
    }
}

impl<In: Clone, Out: Clone, Err: Clone> Operation for FnOperation<In, Out, Err> {
    type In = In;
    type Out = Out;
    type Err = Err;

    async fn execute(&self, input: In) -> Result<Out, Err> {
        (self.execute_fn)(input).await
    }

    async fn compensate(&self, input: In, result: Out) -> Result<(), Err> {
        (self.compensate_fn)(input, result).await
    }
}

/// The result of a transaction execution.
pub type TransactionResult<Out, Err> = Result<Out, TransactionFailure<Err>>;

/// The result of a transaction execution that failed.
#[derive(Debug)]
pub enum TransactionFailure<Err> {
    /// The host policy exhausted its retry budget after complete compensation.
    RetryExhausted(Err),
    /// The host rejected the policy or continuation after complete compensation.
    RetryPolicyError { failure: Err, message: String },
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
            TransactionFailure::RetryExhausted(err) => {
                write!(f, "Transaction retry policy exhausted: {err}")
            }
            TransactionFailure::RetryPolicyError { failure, message } => write!(
                f,
                "Transaction failed with {failure}; retry policy error: {message}"
            ),
            TransactionFailure::FailedAndRolledBackCompletely(err) => {
                write!(
                    f,
                    "Transaction failed with {err} and rolled back completely."
                )
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
/// successful operation's compensation actions are executed in reverse order and the transaction
/// returns with a failure.
pub async fn fallible_transaction<Out, Err>(
    f: impl for<'a> FnOnce(&'a mut FallibleTransaction<Err>) -> LocalBoxFuture<'a, Result<Out, Err>>,
) -> TransactionResult<Out, Err>
where
    Err: Clone + 'static,
{
    let mut transaction = FallibleTransaction::new();
    match f(&mut transaction).await {
        Ok(output) => Ok(output),
        Err(error) => Err(transaction.on_fail(error).await),
    }
}

/// Runs new attempts until the body succeeds. Return operation errors with `?`.
/// Each failed attempt compensates successful operations in reverse order before
/// starting a new attempt. Attempts append durable history; they never rewind it.
/// The host resolves the `saga` policy after each completely compensated failure.
/// Exhaustion, policy errors and partial compensation return typed failures.
/// Rust WASM panics abort as programming errors without compensation.
pub async fn infallible_transaction<Out, Err: Clone + 'static>(
    mut f: impl for<'a> FnMut(&'a mut FallibleTransaction<Err>) -> LocalBoxFuture<'a, Result<Out, Err>>,
) -> TransactionResult<Out, Err> {
    let started = saga_host::now();
    let mut state = Vec::new();
    let mut attempt = 0_i64;
    loop {
        let mut transaction = FallibleTransaction::new();
        match f(&mut transaction).await {
            Ok(output) => return Ok(output),
            Err(error) => {
                let failure = transaction.on_fail(error).await;
                let TransactionFailure::FailedAndRolledBackCompletely(error) = failure else {
                    return Err(failure);
                };
                attempt = attempt.saturating_add(1);
                match saga_next(&state, saga_host::now().saturating_sub(started), attempt) {
                    Ok(decision) => {
                        state = decision.state;
                        match decision.delay {
                            Some(delay) => saga_host::wait(delay).await,
                            None => return Err(TransactionFailure::RetryExhausted(error)),
                        }
                    }
                    Err(message) => {
                        return Err(TransactionFailure::RetryPolicyError {
                            failure: error,
                            message,
                        });
                    }
                }
            }
        }
    }
}

fn saga_next(
    state: &[u8],
    elapsed: u64,
    attempt: i64,
) -> Result<crate::bindings::golem::api::retry::RetryDecision, String> {
    use crate::bindings::golem::api::retry::PredicateValue;
    let noun = saga_host::noun()?;
    let properties = vec![
        ("attempt".to_string(), PredicateValue::Integer(attempt)),
        (
            "error-kind".to_string(),
            PredicateValue::Text("operation-error".to_string()),
        ),
    ];
    let policy = saga_host::resolve("saga", &noun, &properties)?;
    saga_host::next(policy.as_ref(), state, elapsed, "saga", &noun, &properties)
}

#[cfg(test)]
mod saga_host {
    use crate::bindings::golem::api::retry::{PredicateValue, RetryDecision, RetryPolicy};
    use std::cell::Cell;
    thread_local! {
        pub static CLOCK: Cell<u64> = const { Cell::new(0) };
        pub static ELAPSED: Cell<u64> = const { Cell::new(0) };
        pub static POLICY_ERROR: Cell<bool> = const { Cell::new(false) };
        pub static RESOLVE_ERROR: Cell<bool> = const { Cell::new(false) };
    }
    pub fn now() -> u64 {
        CLOCK.get()
    }
    pub async fn wait(delay: u64) {
        CLOCK.set(CLOCK.get() + delay);
    }
    pub fn noun() -> Result<String, String> {
        Ok("urn:worker:component/agent".into())
    }
    pub fn resolve(
        verb: &str,
        noun: &str,
        properties: &[(String, PredicateValue)],
    ) -> Result<Option<RetryPolicy>, String> {
        assert_eq!(verb, "saga");
        assert_eq!(noun, "urn:worker:component/agent");
        assert_eq!(properties[1].0, "error-kind");
        assert!(
            matches!(&properties[1].1, PredicateValue::Text(value) if value == "operation-error")
        );
        if RESOLVE_ERROR.replace(false) {
            return Err("invalid resolver predicate".into());
        }
        Ok(None)
    }
    pub fn next(
        policy: Option<&RetryPolicy>,
        state: &[u8],
        elapsed: u64,
        verb: &str,
        noun: &str,
        properties: &[(String, PredicateValue)],
    ) -> Result<RetryDecision, String> {
        assert!(policy.is_none());
        ELAPSED.set(elapsed);
        resolve(verb, noun, properties)?;
        assert_eq!(properties[0].0, "attempt");
        let PredicateValue::Integer(attempt) = properties[0].1 else {
            panic!("missing attempt")
        };
        if POLICY_ERROR.replace(false) {
            return Err("invalid policy".into());
        }
        let expected = if attempt == 1 {
            vec![]
        } else {
            vec![attempt as u8 - 1]
        };
        assert_eq!(state, expected);
        assert!(elapsed >= (attempt as u64 - 1) * 10);
        Ok(RetryDecision {
            state: vec![attempt as u8],
            delay: (attempt < 3).then_some(10),
        })
    }
}

#[cfg(not(test))]
mod saga_host {
    pub use crate::bindings::golem::api::retry::next_retry_delay as next;
    pub use crate::retry::resolve_retry_policy as resolve;
    pub fn now() -> u64 {
        crate::wasip3::clocks::monotonic_clock::now()
    }
    pub async fn wait(delay: u64) {
        crate::wasip3::clocks::monotonic_clock::wait_for(delay).await;
    }
    pub fn noun() -> Result<String, String> {
        let metadata = crate::get_self_metadata().map_err(|e| format!("{e:?}"))?;
        let id = metadata.agent_id;
        Ok(format!(
            "urn:worker:{}/{}",
            id.component_id.uuid, id.agent_id
        ))
    }
}

/// Runs a transaction through the [`Transaction`] interface.
pub async fn transaction<Out: 'static, Err, T>(
    f: impl for<'a> FnOnce(&'a mut T) -> LocalBoxFuture<'a, Result<Out, Err>>,
) -> TransactionResult<Out, Err>
where
    T: Transaction<Err>,
{
    T::run(f).await
}

/// Helper struct for coupling compensation action and the result of the operation.
#[allow(clippy::type_complexity)]
struct CompensationAction<Err> {
    action: Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = Result<(), Err>>>>>,
}

impl<Err> CompensationAction<Err> {
    pub async fn execute(self) -> Result<(), Err> {
        (self.action)().await
    }
}

/// FallibleTransaction is a sequence of operations that are executed in a way that if any of the
/// operations fails all the already performed operation's compensation actions got executed in
/// reverse order.
///
/// Panics and external executor failures do not trigger compensation. Recovery
/// follows ordinary durable replay, without rewinding the transaction.
pub struct FallibleTransaction<Err> {
    compensations: Vec<CompensationAction<Err>>,
}

impl<Err: Clone + 'static> FallibleTransaction<Err> {
    fn new() -> Self {
        Self {
            compensations: Vec::new(),
        }
    }

    pub async fn execute<OpIn: Clone + 'static, OpOut: Clone + 'static>(
        &mut self,
        operation: impl Operation<In = OpIn, Out = OpOut, Err = Err> + 'static,
        input: OpIn,
    ) -> Result<OpOut, Err> {
        let result = operation.execute(input.clone()).await;
        if let Ok(output) = &result {
            let cloned_op = operation.clone();
            let cloned_out = output.clone();
            self.compensations.push(CompensationAction {
                action: Box::new(move || {
                    Box::pin(async move {
                        cloned_op
                            .compensate(input.clone(), cloned_out.clone())
                            .await
                    })
                }),
            });
        }
        result
    }

    async fn on_fail(&mut self, failure: Err) -> TransactionFailure<Err> {
        for compensation_action in self.compensations.drain(..).rev() {
            if let Err(compensation_failure) = compensation_action.execute().await {
                return TransactionFailure::FailedAndRolledBackPartially {
                    failure,
                    compensation_failure,
                };
            }
        }
        TransactionFailure::FailedAndRolledBackCompletely(failure)
    }
}

/// A unified interface for the different types of transactions. Using it can make the code
/// easier to switch between different transactional guarantees but is more constrained in
/// terms of error types.
#[allow(async_fn_in_trait)]
pub trait Transaction<Err> {
    async fn execute<OpIn: Clone + 'static, OpOut: Clone + 'static>(
        &mut self,
        operation: impl Operation<In = OpIn, Out = OpOut, Err = Err> + 'static,
        input: OpIn,
    ) -> Result<OpOut, Err>;

    async fn fail(&mut self, error: Err) -> Result<(), Err>;

    async fn run<Out: 'static>(
        f: impl for<'a> FnOnce(&'a mut Self) -> LocalBoxFuture<'a, Result<Out, Err>>,
    ) -> TransactionResult<Out, Err>;
}

impl<Err: Clone + 'static> Transaction<Err> for FallibleTransaction<Err> {
    async fn execute<OpIn: Clone + 'static, OpOut: Clone + 'static>(
        &mut self,
        operation: impl Operation<In = OpIn, Out = OpOut, Err = Err> + 'static,
        input: OpIn,
    ) -> Result<OpOut, Err> {
        FallibleTransaction::execute(self, operation, input).await
    }

    async fn fail(&mut self, error: Err) -> Result<(), Err> {
        Err(error)
    }

    async fn run<Out: 'static>(
        f: impl for<'a> FnOnce(&'a mut Self) -> LocalBoxFuture<'a, Result<Out, Err>>,
    ) -> TransactionResult<Out, Err> {
        fallible_transaction(f).await
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use test_r::test;

    use crate::{boxed, fallible_transaction, infallible_transaction, sync_operation};

    #[test]
    async fn bounded_retries_wait_and_exhaust() {
        super::saga_host::CLOCK.set(0);
        let op = sync_operation(
            |()| {
                super::saga_host::CLOCK.set(super::saga_host::CLOCK.get() + 2);
                Ok::<_, &str>(())
            },
            |(), ()| {
                super::saga_host::CLOCK.set(super::saga_host::CLOCK.get() + 3);
                Ok(())
            },
        );
        let mut attempts = 0;
        let result = infallible_transaction(|tx| {
            attempts += 1;
            let op = op.clone();
            boxed(async move {
                tx.execute(op, ()).await?;
                Err::<(), _>("operation")
            })
        })
        .await;
        assert!(matches!(
            result,
            Err(crate::TransactionFailure::RetryExhausted("operation"))
        ));
        assert_eq!(attempts, 3);
        assert_eq!(super::saga_host::CLOCK.get(), 35);
        assert_eq!(super::saga_host::ELAPSED.get(), 35);
    }

    #[test]
    async fn resolver_error_preserves_original_failure() {
        super::saga_host::RESOLVE_ERROR.set(true);
        let mut attempts = 0;
        let result = infallible_transaction(|_| {
            attempts += 1;
            boxed(async { Err::<(), _>("original") })
        })
        .await;
        assert!(matches!(
            result,
            Err(crate::TransactionFailure::RetryPolicyError { failure: "original", message })
                if message == "invalid resolver predicate"
        ));
        assert_eq!(attempts, 1);
    }

    #[test]
    async fn invalid_policy_never_retries() {
        super::saga_host::POLICY_ERROR.set(true);
        let mut attempts = 0;
        let result = infallible_transaction(|_| {
            attempts += 1;
            boxed(async { Err::<(), _>("operation") })
        })
        .await;
        assert!(matches!(
            result,
            Err(crate::TransactionFailure::RetryPolicyError {
                failure: "operation",
                ..
            })
        ));
        assert_eq!(attempts, 1);
    }

    #[test]
    async fn compensation_order_and_partial_failure() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let compensation_log = log.clone();
        let op = sync_operation(
            |input: u8| Ok::<_, &str>(input),
            move |input, output| {
                assert_eq!(input, output);
                compensation_log.borrow_mut().push(input);
                if input == 2 {
                    Err("compensation")
                } else {
                    Ok(())
                }
            },
        );
        let result = fallible_transaction(|tx| {
            boxed(async move {
                tx.execute(op.clone(), 1).await?;
                tx.execute(op.clone(), 2).await?;
                tx.execute(op, 3).await?;
                Err::<(), _>("body")
            })
        })
        .await;
        assert!(matches!(
            result,
            Err(crate::TransactionFailure::FailedAndRolledBackPartially {
                failure: "body",
                compensation_failure: "compensation"
            })
        ));
        assert_eq!(*log.borrow(), [3, 2]);
    }

    #[test]
    async fn infallible_stops_on_compensation_failure() {
        let op = sync_operation(|()| Ok::<_, &str>(()), |(), ()| Err("compensation"));
        let mut attempts = 0;
        let result = infallible_transaction(|tx| {
            attempts += 1;
            assert_eq!(attempts, 1);
            let op = op.clone();
            boxed(async move {
                tx.execute(op, ()).await?;
                Err::<(), _>("body")
            })
        })
        .await;
        assert!(matches!(
            result,
            Err(crate::TransactionFailure::FailedAndRolledBackPartially {
                failure: "body",
                compensation_failure: "compensation"
            })
        ));
    }

    #[test]
    async fn fallible_compensates_successful_operations_only() {
        let log = Rc::new(RefCell::new(Vec::new()));

        let log1 = log.clone();
        let log2 = log.clone();
        let log3 = log.clone();
        let log4 = log.clone();

        let op1 = sync_operation(
            move |input: String| {
                log1.borrow_mut().push(format!("op1 execute {input}"));
                Ok(())
            },
            move |input: String, _| {
                log2.borrow_mut().push(format!("op1 rollback {input}"));
                Ok(())
            },
        );

        let op2 = sync_operation(
            move |_: ()| {
                log3.clone().borrow_mut().push("op2 execute".to_string());
                Err::<(), &str>("op2 error")
            },
            move |_: (), _| {
                log4.clone().borrow_mut().push("op2 rollback".to_string());
                Ok(())
            },
        );

        let result = fallible_transaction(|tx| {
            boxed(async move {
                println!("First we execute op1");
                tx.execute(op1, "hello".to_string()).await?;
                println!("Then execute op2");
                tx.execute(op2, ()).await?;
                println!("Finally compute a result");
                Ok(11)
            })
        })
        .await;

        assert!(matches!(
            result,
            Err(crate::TransactionFailure::FailedAndRolledBackCompletely(
                "op2 error"
            ))
        ));
        assert_eq!(
            *log.borrow(),
            ["op1 execute hello", "op2 execute", "op1 rollback hello"]
        );
    }

    #[test]
    async fn infallible_retries_with_fresh_compensations() {
        let log = Rc::new(RefCell::new(Vec::new()));

        let log1 = log.clone();
        let log2 = log.clone();
        let log3 = log.clone();
        let log4 = log.clone();

        let op1 = sync_operation(
            move |input: String| {
                log1.borrow_mut().push(format!("op1 execute {input}"));
                Ok::<(), &str>(())
            },
            move |input: String, _| {
                log2.borrow_mut().push(format!("op1 rollback {input}"));
                Ok(())
            },
        );

        let attempts = Rc::new(std::cell::Cell::new(0));
        let count = attempts.clone();
        let op2 = sync_operation(
            move |_: ()| {
                log3.clone().borrow_mut().push("op2 execute".to_string());
                count.set(count.get() + 1);
                if count.get() < 3 {
                    Err("op2 error")
                } else {
                    Ok(())
                }
            },
            move |_: (), r| {
                log4.clone()
                    .borrow_mut()
                    .push(format!("op2 rollback {r:?}"));
                Ok(())
            },
        );

        let result = infallible_transaction(|tx| {
            let op1 = op1.clone();
            let op2 = op2.clone();
            boxed(async move {
                println!("First we execute op1");
                tx.execute(op1.clone(), "hello".to_string()).await?;
                tx.execute(op1, "world".to_string()).await?;
                println!("Then execute op2");
                tx.execute(op2, ()).await?;
                println!("Finally compute a result");
                Ok(11)
            })
        })
        .await;

        assert_eq!(result.unwrap(), 11);
        assert_eq!(attempts.get(), 3);
        assert_eq!(
            *log.borrow(),
            [
                "op1 execute hello",
                "op1 execute world",
                "op2 execute",
                "op1 rollback world",
                "op1 rollback hello",
                "op1 execute hello",
                "op1 execute world",
                "op2 execute",
                "op1 rollback world",
                "op1 rollback hello",
                "op1 execute hello",
                "op1 execute world",
                "op2 execute"
            ]
        );
    }
}

#[cfg(test)]
#[cfg(feature = "macro")]
mod macro_tests {
    use crate::{boxed, fallible_transaction, infallible_transaction};
    use golem_rust_macro::golem_operation;
    use test_r::test;

    mod golem_rust {
        pub use crate::*;
    }

    #[golem_operation(compensation=test_compensation)]
    fn test_operation(input1: u64, input2: f32) -> Result<bool, String> {
        println!("Op input: {input1}, {input2}");
        Ok(true)
    }

    fn test_compensation(_: bool, input1: u64, input2: f32) -> Result<(), String> {
        println!("Compensation input: {input1}, {input2}");
        Ok(())
    }

    #[golem_operation(compensation=test_compensation_2)]
    fn test_operation_2(input1: u64, input2: f32) -> Result<bool, String> {
        println!("Op input: {input1}, {input2}");
        Ok(true)
    }

    fn test_compensation_2(result: bool) -> Result<(), String> {
        println!("Compensation for operation result {result:?}");
        Ok(())
    }

    #[golem_operation(compensation=test_compensation_3)]
    fn test_operation_3(input: String) -> Result<(), String> {
        println!("Op input: {input}");
        Ok(())
    }

    fn test_compensation_3() -> Result<(), String> {
        println!("Compensation for operation, not using any input");
        Ok(())
    }

    #[golem_operation(compensation=test_compensation_4)]
    fn test_operation_4(input: u64) -> Result<(), String> {
        println!("Op input: {input}");
        Ok(())
    }

    fn test_compensation_4(_: (), input: u64) -> Result<(), String> {
        println!("Compensation for operation with single input {input}");
        Ok(())
    }

    // Not a real test, just verifying that the code compiles
    #[test]
    #[ignore]
    async fn tx_test_1() {
        let result = fallible_transaction(|tx| {
            boxed(async move {
                println!("Executing the annotated function as an operation directly");
                tx.test_operation(1, 0.1).await?;
                tx.test_operation_2(1, 0.1).await?;
                tx.test_operation_3("test".to_string()).await?;
                tx.test_operation_4(1).await?;

                Ok(11)
            })
        })
        .await;

        println!("{result:?}");
    }

    // Not a real test, just verifying that the code compiles
    #[test]
    #[ignore]
    async fn tx_test_2() {
        let result = infallible_transaction(|tx| {
            boxed(async move {
                println!("Executing the annotated function as an operation directly");
                tx.test_operation(1, 0.1).await?;
                Ok(11)
            })
        })
        .await;

        println!("{result:?}");
    }
}
