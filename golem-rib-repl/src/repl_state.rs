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
use std::sync::{Arc, Mutex, RwLock};

pub struct ReplState {
    dependency: RibComponentMetadata,
    raw_rib_script: RwLock<Vec<String>>,
    worker_function_invoke: Arc<dyn WorkerFunctionInvoke + Sync + Send>,
    invocation_results: InvocationResultCache,
}

impl ReplState {
    pub fn worker_function_invoke(&self) -> &Arc<dyn WorkerFunctionInvoke + Sync + Send> {
        &self.worker_function_invoke
    }

    pub fn invocation_results(&self) -> &InvocationResultCache {
        &self.invocation_results
    }

    pub fn update_result(&self, instruction_id: &InstructionId, result: ValueAndType) {
        self.invocation_results
            .results
            .lock()
            .unwrap()
            .insert(instruction_id.clone(), result);
    }

    pub fn current_rib_program(&self) -> String {
        self.raw_rib_script.read().unwrap().join(";\n")
    }

    pub fn update_rib(&self, rib: &str) {
        self.raw_rib_script.write().unwrap().push(rib.to_string());
    }

    pub fn update_dependency(&mut self, dependency: RibComponentMetadata) {
        self.dependency = dependency;
    }

    pub fn pop_rib_text(&self) {
        self.raw_rib_script.write().unwrap().pop();
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
            raw_rib_script: RwLock::new(Vec::new()),
            worker_function_invoke,
            invocation_results: InvocationResultCache {
                results: Mutex::new(HashMap::new()),
            },
        }
    }
}

#[derive(Debug)]
pub struct InvocationResultCache {
    pub results: Mutex<HashMap<InstructionId, ValueAndType>>,
}

impl InvocationResultCache {
    pub fn get(&self, script_id: &InstructionId) -> Option<ValueAndType> {
        self.results.lock().unwrap().get(script_id).cloned()
    }
}
