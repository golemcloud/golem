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

use crate::invoke::ReplRibFunctionInvoke;
use crate::repl_state::ReplState;
use rib::{InstructionId, Interpreter, RibByteCode, RibInput, RibResult, RibRuntimeError};
use std::sync::Arc;
use crate::worker_name_gen::DynamicWorkerGen;

pub async fn eval(
    rib_byte_code: RibByteCode,
    repl_state: &Arc<ReplState>,
) -> Result<RibResult, RibRuntimeError> {
    let last_instruction = InstructionId::new(rib_byte_code.len());

    let rib_result = dynamic_interpreter(repl_state).run(rib_byte_code).await?;

    repl_state.update_last_executed_instruction(last_instruction);

    Ok(rib_result)
}

// A dynamic rib interpreter that is created based on the state of the repl
fn dynamic_interpreter(repl_state: &Arc<ReplState>) -> Interpreter {
    let rib_function_invoke = Arc::new(ReplRibFunctionInvoke::new(repl_state.clone()));
    let worker_name_generator = Arc::new(DynamicWorkerGen::new(repl_state.clone()));

    Interpreter::new(RibInput::default(), rib_function_invoke, worker_name_generator)
}
