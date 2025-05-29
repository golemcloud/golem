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

use crate::interpreter::interpreter_stack_value::RibInterpreterStackValue;
use crate::{
    EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, InstructionId, RibFunctionInvoke,
    RibInput, VariableId,
};
use golem_wasm_rpc::ValueAndType;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;

pub struct InterpreterEnv {
    pub env: HashMap<EnvironmentKey, RibInterpreterStackValue>,
    pub call_worker_function_async: Arc<dyn RibFunctionInvoke + Sync + Send>,
}

impl Debug for InterpreterEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InterpreterEnv")
            .field("env", &self.env.iter())
            .finish()
    }
}

impl Default for InterpreterEnv {
    fn default() -> Self {
        InterpreterEnv {
            env: HashMap::new(),
            call_worker_function_async: Arc::new(internal::NoopRibFunctionInvoke),
        }
    }
}

impl InterpreterEnv {
    pub async fn invoke_worker_function_async(
        &self,
        instruction_id: &InstructionId,
        worker_name: Option<String>,
        function_name: String,
        args: Vec<ValueAndType>,
    ) -> Result<ValueAndType, Box<dyn std::error::Error + Send + Sync>> {
        self.call_worker_function_async
            .invoke(
                instruction_id,
                worker_name.map(EvaluatedWorkerName),
                EvaluatedFqFn(function_name),
                EvaluatedFnArgs(args),
            )
            .await
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
            call_worker_function_async: Arc::new(internal::NoopRibFunctionInvoke),
        }
    }

    pub fn from(
        input: &RibInput,
        call_worker_function_async: &Arc<dyn RibFunctionInvoke + Sync + Send>,
    ) -> Self {
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
    use crate::{EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, InstructionId};
    use async_trait::async_trait;
    use golem_wasm_ast::analysis::analysed_type::tuple;
    use golem_wasm_rpc::{Value, ValueAndType};

    pub(crate) struct NoopRibFunctionInvoke;

    #[async_trait]
    impl RibFunctionInvoke for NoopRibFunctionInvoke {
        async fn invoke(
            &self,
            _instruction_id: &InstructionId,
            _worker_name: Option<EvaluatedWorkerName>,
            _function_name: EvaluatedFqFn,
            _args: EvaluatedFnArgs,
        ) -> Result<ValueAndType, Box<dyn std::error::Error + Send + Sync>> {
            Ok(ValueAndType::new(Value::Tuple(vec![]), tuple(vec![])))
        }
    }
}
