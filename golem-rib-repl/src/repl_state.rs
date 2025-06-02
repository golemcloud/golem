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
use crate::{RawRibScript, WorkerFunctionInvoke};
use golem_wasm_rpc::ValueAndType;
use rib::{InstructionId, RibCompiler};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock, RwLockReadGuard};

pub struct ReplState {
    // https://github.com/golemcloud/golem/issues/1608 will avoid having to keep
    // dependency separately in the ReplState
    dependency: RibComponentMetadata,
    rib_script: RwLock<RawRibScript>,
    worker_function_invoke: Arc<dyn WorkerFunctionInvoke + Sync + Send>,
    invocation_results: InvocationResultCache,
    last_executed_instruction: RwLock<Option<InstructionId>>,
    rib_compiler: RwLock<RibCompiler>,
    history_file_path: PathBuf,
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

    pub fn history_file_path(&self) -> &PathBuf {
        &self.history_file_path
    }

    pub fn update_last_executed_instruction(&self, instruction_id: InstructionId) {
        *self.last_executed_instruction.write().unwrap() = Some(instruction_id);
    }

    pub fn clear(&self) {
        *self.rib_script.write().unwrap() = RawRibScript::default();
        *self.invocation_results.results.write().unwrap() = HashMap::new();
        *self.last_executed_instruction.write().unwrap() = None;
    }

    pub fn rib_script(&self) -> RwLockReadGuard<RawRibScript> {
        self.rib_script.read().unwrap()
    }

    pub fn rib_compiler(&self) -> RwLockReadGuard<RibCompiler> {
        self.rib_compiler.read().unwrap()
    }

    pub fn current_rib_program(&self) -> String {
        self.rib_script.read().unwrap().as_text()
    }

    pub fn update_rib(&self, rib: &str) {
        self.rib_script.write().unwrap().push(rib);
    }

    pub fn remove_last_rib_expression(&self) {
        self.rib_script.write().unwrap().pop();
    }

    pub fn dependency(&self) -> &RibComponentMetadata {
        &self.dependency
    }

    pub fn new(
        dependency: RibComponentMetadata,
        worker_function_invoke: Arc<dyn WorkerFunctionInvoke + Sync + Send>,
        rib_compiler: RibCompiler,
        history_file: PathBuf,
    ) -> Self {
        Self {
            dependency,
            rib_script: RwLock::new(RawRibScript::default()),
            worker_function_invoke,
            invocation_results: InvocationResultCache {
                results: RwLock::new(HashMap::new()),
            },
            last_executed_instruction: RwLock::new(None),
            rib_compiler: RwLock::new(rib_compiler),
            history_file_path: history_file,
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
