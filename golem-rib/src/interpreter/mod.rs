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
