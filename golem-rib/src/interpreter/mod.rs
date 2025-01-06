// Copyright 2024-2025 Golem Cloud
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

pub use env::RibFunctionInvoke;
pub use interpreter_input::*;
pub use interpreter_result::*;
pub use literal::*;

use crate::interpreter::rib_interpreter::Interpreter;
use crate::RibByteCode;

mod env;
mod instruction_cursor;
mod interpreter_input;
mod interpreter_result;
mod interpreter_stack_value;
mod literal;
mod rib_interpreter;
mod stack;
mod tests;

pub async fn interpret(
    rib: &RibByteCode,
    rib_input: &RibInput,
    function_invoke: RibFunctionInvoke,
) -> Result<RibResult, String> {
    let mut interpreter = Interpreter::new(rib_input, function_invoke);
    interpreter.run(rib.clone()).await
}

// This function can be used for those the Rib Scripts
// where there are no side effecting function calls.
// It is recommended to use `interpret` over `interpret_pure` if you are unsure.
pub async fn interpret_pure(rib: &RibByteCode, rib_input: &RibInput) -> Result<RibResult, String> {
    let mut interpreter = Interpreter::pure(rib_input);
    interpreter.run(rib.clone()).await
}
