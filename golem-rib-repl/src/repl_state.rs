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

use crate::dependency_manager::RibComponentMetadata;
use rib::{
    Interpreter, InterpreterEnv, InterpreterStack, RibByteCode, RibFunctionInvoke, RibInput,
};
use std::sync::Arc;

pub struct ReplState {
    byte_code: RibByteCode,
    interpreter: Interpreter,
    dependency: RibComponentMetadata,
    rib_code_collection: Vec<String>,
}

impl ReplState {
    pub fn current_rib_program(&self) -> String {
        self.rib_code_collection.join(";")
    }

    pub fn update_rib(&mut self, rib: &str) {
        self.rib_code_collection.push(rib.to_string());
    }

    pub fn update_dependency(&mut self, dependency: RibComponentMetadata) {
        self.dependency = dependency;
    }

    pub fn pop_rib_text(&mut self) {
        self.rib_code_collection.pop();
    }

    pub fn interpreter(&mut self) -> &mut Interpreter {
        &mut self.interpreter
    }
    pub fn byte_code(&self) -> &RibByteCode {
        &self.byte_code
    }

    pub fn update_byte_code(&mut self, byte_code: RibByteCode) {
        self.byte_code = byte_code;
    }

    pub fn dependency(&self) -> &RibComponentMetadata {
        &self.dependency
    }

    pub fn new(
        dependency: &RibComponentMetadata,
        invoke: Arc<dyn RibFunctionInvoke + Sync + Send>,
    ) -> Self {
        let interpreter_env = InterpreterEnv::from(&RibInput::default(), &invoke);

        Self {
            byte_code: RibByteCode::default(),
            interpreter: Interpreter::new(
                &RibInput::default(),
                invoke,
                Some(InterpreterStack::default()),
                Some(interpreter_env),
            ),
            dependency: dependency.clone(),
            rib_code_collection: vec![],
        }
    }
}
