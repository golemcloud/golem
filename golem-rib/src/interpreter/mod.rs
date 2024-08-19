pub use env::RibFunctionInvoke;
pub use literal::*;
pub use result::*;
pub use rib_interpreter::*;

use crate::RibByteCode;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use std::collections::HashMap;

mod env;
mod literal;
mod result;
mod rib_interpreter;
mod stack;

pub async fn interpret(
    rib: &RibByteCode,
    rib_input: HashMap<String, TypeAnnotatedValue>,
    function_invoke: RibFunctionInvoke,
) -> Result<RibInterpreterResult, String> {
    let mut interpreter = Interpreter::new(rib_input, function_invoke);
    interpreter.run(rib.clone()).await
}
