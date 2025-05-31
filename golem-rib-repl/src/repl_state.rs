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

use crate::dependency_manager::RibComponentMetadata;
use crate::WorkerFunctionInvoke;
use golem_wasm_rpc::ValueAndType;
use rib::InstructionId;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub struct ReplState {
    dependency: RibComponentMetadata,
    rib_script: RwLock<RawRibScript>,
    worker_function_invoke: Arc<dyn WorkerFunctionInvoke + Sync + Send>,
    invocation_results: InvocationResultCache,
    last_executed_instruction: RwLock<Option<InstructionId>>,
}

impl ReplState {
    pub fn worker_function_invoke(&self) -> &Arc<dyn WorkerFunctionInvoke + Sync + Send> {
        &self.worker_function_invoke
    }

    pub fn invocation_results(&self) -> &InvocationResultCache {
        &self.invocation_results
    }

    pub fn update_cache(&self, instruction_id: InstructionId, result: ValueAndType) {
        self.invocation_results
            .results
            .write()
            .unwrap()
            .insert(instruction_id, result);
    }

    pub fn last_executed_instruction(&self) -> InstructionId {
        self.last_executed_instruction
            .read()
            .unwrap()
            .clone()
            .unwrap_or(InstructionId { index: 0 })
    }

    pub fn update_last_executed_instruction(&self, instruction_id: InstructionId) {
        *self.last_executed_instruction.write().unwrap() = Some(instruction_id);
    }

    pub fn current_rib_program(&self) -> String {
        self.rib_script.read().unwrap().as_text()
    }

    pub fn update_rib(&self, rib: &str) {
        self.rib_script.write().unwrap().push(rib);
    }

    pub fn update_dependency(&mut self, dependency: RibComponentMetadata) {
        self.dependency = dependency;
    }

    pub fn remove_last_rib_expression(&self) {
        self.rib_script.write().unwrap().pop();
    }

    pub fn dependency(&self) -> &RibComponentMetadata {
        &self.dependency
    }

    pub fn new(
        dependency: &RibComponentMetadata,
        worker_function_invoke: Arc<dyn WorkerFunctionInvoke + Sync + Send>,
    ) -> Self {
        Self {
            dependency: dependency.clone(),
            rib_script: RwLock::new(RawRibScript::default()),
            worker_function_invoke,
            invocation_results: InvocationResultCache {
                results: RwLock::new(HashMap::new()),
            },
            last_executed_instruction: RwLock::new(None),
        }
    }
}

#[derive(Debug)]
pub struct InvocationResultCache {
    pub results: RwLock<HashMap<InstructionId, ValueAndType>>,
}

impl InvocationResultCache {
    pub fn get(&self, script_id: &InstructionId) -> Option<ValueAndType> {
        self.results.read().unwrap().get(script_id).cloned()
    }
}

#[derive(Default)]
pub struct RawRibScript {
    value: Vec<String>,
}

impl RawRibScript {
    pub fn push(&mut self, rib: &str) {
        self.value.push(rib.to_string());
    }

    pub fn pop(&mut self) {
        self.value.pop();
    }

    pub fn as_text(&self) -> String {
        self.value.join(";\n")
    }
}
