use crate::invoke::ReplRibFunctionInvoke;
use crate::repl_state::ReplState;
use rib::{InstructionId, Interpreter, RibByteCode, RibInput, RibResult, RibRuntimeError};
use std::sync::Arc;

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

    Interpreter::new(RibInput::default(), rib_function_invoke)
}
