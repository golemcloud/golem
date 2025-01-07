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

use crate::interpreter::interpreter_stack_value::RibInterpreterStackValue;
use crate::{RibInput, VariableId};
use golem_wasm_rpc::ValueAndType;
use std::collections::HashMap;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub struct InterpreterEnv {
    pub env: HashMap<EnvironmentKey, RibInterpreterStackValue>,
    pub call_worker_function_async: RibFunctionInvoke,
}

impl Debug for InterpreterEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InterpreterEnv")
            .field("env", &self.env.iter())
            .finish()
    }
}

pub type RibFunctionInvoke = Arc<
    dyn Fn(
            String,
            Vec<ValueAndType>,
        ) -> Pin<Box<dyn Future<Output = Result<ValueAndType, String>> + Send>>
        + Send
        + Sync,
>;

impl Default for InterpreterEnv {
    fn default() -> Self {
        InterpreterEnv {
            env: HashMap::new(),
            call_worker_function_async: internal::default_worker_invoke_async(),
        }
    }
}

impl InterpreterEnv {
    pub fn invoke_worker_function_async(
        &self,
        function_name: String,
        args: Vec<ValueAndType>,
    ) -> Pin<Box<dyn Future<Output = Result<ValueAndType, String>> + Send>> {
        (self.call_worker_function_async)(function_name, args)
    }

    pub fn from_input(env: &RibInput) -> Self {
        let env = env
            .input
            .clone()
            .into_iter()
            .map(|(k, v)| {
                (
                    EnvironmentKey::from_global(k),
                    RibInterpreterStackValue::Val(v),
                )
            })
            .collect();

        InterpreterEnv {
            env,
            call_worker_function_async: internal::default_worker_invoke_async(),
        }
    }

    pub fn from(input: &RibInput, call_worker_function_async: &RibFunctionInvoke) -> Self {
        let mut env = Self::from_input(input);
        env.call_worker_function_async = call_worker_function_async.clone();
        env
    }

    pub fn insert(&mut self, key: EnvironmentKey, value: RibInterpreterStackValue) {
        self.env.insert(key, value);
    }

    pub fn lookup(&self, key: &EnvironmentKey) -> Option<&RibInterpreterStackValue> {
        self.env.get(key)
    }
}

#[derive(Debug, Hash, Eq, PartialEq, Clone)]
pub struct EnvironmentKey {
    pub variable_id: VariableId,
}

impl EnvironmentKey {
    pub fn from(variable_id: VariableId) -> Self {
        EnvironmentKey { variable_id }
    }

    pub fn from_global(key: String) -> Self {
        EnvironmentKey {
            variable_id: VariableId::global(key),
        }
    }
}

mod internal {
    use crate::interpreter::env::RibFunctionInvoke;
    use golem_wasm_ast::analysis::analysed_type::tuple;
    use golem_wasm_rpc::{Value, ValueAndType};
    use std::sync::Arc;

    pub(crate) fn default_worker_invoke_async() -> RibFunctionInvoke {
        Arc::new(|_, _| {
            Box::pin(async { Ok(ValueAndType::new(Value::Tuple(vec![]), tuple(vec![]))) })
        })
    }
}
