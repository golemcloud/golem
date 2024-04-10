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

use crate::bindings::golem::api::host::{get_oplog_index, set_oplog_index, OplogIndex};
use crate::mark_atomic_operation;
use std::rc::Rc;

/// Represents an atomic operation of the transaction which has a rollback action.
///
/// Implement this trait and use it within a `transaction` block.
/// Operations can also be constructed from closures using `operation`.
pub trait Operation<In, Out, Err>: Clone {
    fn execute(&self, input: In) -> Result<Out, Err>;
    fn rollback(&self, input: In);
}

/// Constructs an `Operation` from two closures: one for executing the operation,
/// and one for rolling it back
pub fn operation<In, Out, Err>(
    execute_fn: impl Fn(In) -> Result<Out, Err> + 'static,
    rollback_fn: impl Fn(In) + 'static,
) -> impl Operation<In, Out, Err> {
    FnOperation {
        execute_fn: Rc::new(execute_fn),
        rollback_fn: Rc::new(rollback_fn),
    }
}

struct FnOperation<In, Out, Err> {
    execute_fn: Rc<dyn Fn(In) -> Result<Out, Err>>,
    rollback_fn: Rc<dyn Fn(In)>,
}

impl<In, Out, Err> Clone for FnOperation<In, Out, Err> {
    fn clone(&self) -> Self {
        Self {
            execute_fn: self.execute_fn.clone(),
            rollback_fn: self.rollback_fn.clone(),
        }
    }
}

impl<In, Out, Err> Operation<In, Out, Err> for FnOperation<In, Out, Err> {
    fn execute(&self, input: In) -> Result<Out, Err> {
        (self.execute_fn)(input)
    }

    fn rollback(&self, input: In) {
        (self.rollback_fn)(input)
    }
}

/// Transaction is a sequence of operations that are executed in a way that if any of the
/// operations or the underlying Golem executor fails, the whole transaction is going to
/// be retried.
///
/// In addition to that, **user level failures** (represented by the `Result::Err` value
/// of an operation) lead to performing the rollback actions of each already performed operation
/// in reverse order.
///
/// Fatal errors (panic) and external executor failures are currently cannot perform the
/// rollback actions.
pub struct Transaction {
    begin_oplog_index: OplogIndex,
    rollback_actions: Vec<Box<dyn FnOnce()>>,
}

impl Transaction {
    fn new(begin_oplog_index: OplogIndex) -> Self {
        Self {
            begin_oplog_index,
            rollback_actions: Vec::new(),
        }
    }

    pub fn add<OpIn: Clone + 'static, OpOut, OpErr>(
        &mut self,
        operation: impl Operation<OpIn, OpOut, OpErr> + 'static,
        input: OpIn,
    ) -> OpOut {
        let cloned_op = operation.clone();
        let cloned_in = input.clone();
        self.rollback_actions.push(Box::new(move || {
            cloned_op.rollback(cloned_in);
        }));
        match operation.execute(input) {
            Ok(output) => output,
            Err(_) => {
                self.fail();
                unreachable!()
            }
        }
    }

    pub fn fail(&mut self) {
        for rollback_action in self.rollback_actions.drain(..).rev() {
            rollback_action();
        }
        set_oplog_index(self.begin_oplog_index);
    }
}

pub fn transaction<Out>(f: impl FnOnce(&mut Transaction) -> Out) -> Out {
    let oplog_index = get_oplog_index();
    let _atomic_region = mark_atomic_operation();
    let mut transaction = Transaction::new(oplog_index);
    f(&mut transaction)
}

#[cfg(test)]
mod tests {
    use crate::{operation, transaction};
    use std::cell::RefCell;
    use std::rc::Rc;

    // Not a real test, just verifying that the code compiles
    #[test]
    #[ignore]
    fn tx_test_1() {
        let log = Rc::new(RefCell::new(Vec::new()));

        let log1 = log.clone();
        let log2 = log.clone();
        let log3 = log.clone();
        let log4 = log.clone();

        let op1 = operation::<String, (), ()>(
            move |input: String| {
                log1.borrow_mut().push(format!("op1 execute {input}"));
                Ok(())
            },
            move |input: String| {
                log2.borrow_mut().push(format!("op1 rollback {input}"));
            },
        );

        let op2 = operation::<(), (), &str>(
            move |_: ()| {
                log3.clone().borrow_mut().push("op2 execute".to_string());
                Err("op2 error")
            },
            move |_: ()| {
                log4.clone().borrow_mut().push("op2 rollback".to_string());
            },
        );

        let result = transaction(|tx| {
            tx.add(op1, "hello".to_string());
            tx.add(op2, ());
            11
        });

        println!("{log:?}");
        println!("{result:?}");
    }
}
