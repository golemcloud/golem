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

use crate::repl_state::ReplState;
use async_trait::async_trait;
use golem_wasm_rpc::ValueAndType;
use rib::{
    EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, InstructionId, RibFunctionInvoke,
    RibFunctionInvokeResult,
};
use std::sync::Arc;
use uuid::Uuid;

#[async_trait]
pub trait WorkerFunctionInvoke {
    async fn invoke(
        &self,
        component_id: Uuid,
        component_name: &str,
        worker_name: Option<String>,
        function_name: &str,
        args: Vec<ValueAndType>,
    ) -> anyhow::Result<ValueAndType>;
}

// Note: Currently, the Rib interpreter supports only one component, so the
// `RibFunctionInvoke` trait in the `golem-rib` module does not include `component_id` in
// the `invoke` arguments. It only requires the optional worker name, function name, and arguments.
// Once multi-component support is added, the trait will be updated to include `component_id`,
// and we can use it directly instead of `WorkerFunctionInvoke` in the `golem-rib-repl` module.
pub(crate) struct ReplRibFunctionInvoke {
    repl_state: Arc<ReplState>,
}

impl ReplRibFunctionInvoke {
    pub fn new(repl_state: Arc<ReplState>) -> Self {
        Self { repl_state }
    }

    fn get_cached_result(&self, instruction_id: &InstructionId) -> Option<ValueAndType> {
        // If the current instruction index is greater than the last played index result,
        // then we shouldn't use the cache result no matter what.
        // This check is important because without this, loops end up reusing the cached invocation result
        if instruction_id.index > self.repl_state.last_executed_instruction().index {
            None
        } else {
            self.repl_state.invocation_results().get(instruction_id)
        }
    }
}

#[async_trait]
impl RibFunctionInvoke for ReplRibFunctionInvoke {
    async fn invoke(
        &self,
        instruction_id: &InstructionId,
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        args: EvaluatedFnArgs,
    ) -> RibFunctionInvokeResult {
        let component_id = self.repl_state.dependency().component_id;
        let component_name = &self.repl_state.dependency().component_name;

        match self.get_cached_result(instruction_id) {
            Some(result) => Ok(result),
            None => {
                let rib_invocation_result = self
                    .repl_state
                    .worker_function_invoke()
                    .invoke(
                        component_id,
                        component_name,
                        worker_name.map(|x| x.0),
                        function_name.0.as_str(),
                        args.0,
                    )
                    .await;

                match rib_invocation_result {
                    Ok(result) => {
                        self.repl_state
                            .update_cache(instruction_id.clone(), result.clone());

                        Ok(result)
                    }
                    Err(err) => Err(err.into()),
                }
            }
        }
    }
}
