// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub use env::*;
pub use interpreter_input::*;
pub use interpreter_result::*;
pub use literal::*;
pub use rib_function_invoke::*;
pub use rib_interpreter::*;
pub use rib_runtime_error::*;
pub use stack::*;

mod env;
mod instruction_cursor;
mod interpreter_input;
mod interpreter_result;
mod interpreter_stack_value;
mod literal;
mod rib_function_invoke;
mod rib_interpreter;
mod rib_runtime_error;
mod stack;

use crate::RibByteCode;
use std::sync::Arc;

pub async fn interpret(
    rib: RibByteCode,
    rib_input: RibInput,
    function_invoke: Arc<dyn RibFunctionInvoke + Sync + Send>,
) -> Result<RibResult, RibRuntimeError> {
    let mut interpreter = Interpreter::new(rib_input, function_invoke);
    interpreter.run(rib).await
}

// This function can be used for those the Rib Scripts
// where there are no side effecting function calls.
// It is recommended to use `interpret` over `interpret_pure` if you are unsure.
pub async fn interpret_pure(
    rib: RibByteCode,
    rib_input: RibInput,
) -> Result<RibResult, RibRuntimeError> {
    let mut interpreter = Interpreter::pure(rib_input);
    interpreter.run(rib.clone()).await
}

#[macro_export]
macro_rules! internal_corrupted_state {
    ($fmt:expr) => {{
        $crate::interpreter::rib_runtime_error::RibRuntimeError::InvariantViolation($crate::interpreter::rib_runtime_error::InvariantViolation::InternalCorruptedState($fmt.to_string()))
    }};

    ($fmt:expr, $($arg:tt)*) => {{
        $crate::interpreter::rib_runtime_error::RibRuntimeError::InvariantViolation($crate::interpreter::rib_runtime_error::InvariantViolation::InternalCorruptedState(format!($fmt, $($arg)*)))
    }};
}

#[macro_export]
macro_rules! bail_corrupted_state {
    ($fmt:expr) => {{
        return Err($crate::interpreter::rib_runtime_error::RibRuntimeError::InvariantViolation($crate::interpreter::rib_runtime_error::InvariantViolation::InternalCorruptedState($fmt.to_string())));
    }};

    ($fmt:expr, $($arg:tt)*) => {{
        return Err($crate::interpreter::rib_runtime_error::RibRuntimeError::InvariantViolation($crate::interpreter::rib_runtime_error::InvariantViolation::InternalCorruptedState(format!($fmt, $($arg)*))));
    }};
}
