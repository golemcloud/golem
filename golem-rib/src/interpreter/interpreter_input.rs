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

use golem_wasm_rpc::ValueAndType;
use std::collections::HashMap;

// Acts as the structure to hold the global input values
#[derive(Debug, Default, Clone)]
pub struct RibInput {
    pub input: HashMap<String, ValueAndType>,
}

impl RibInput {
    pub fn new(input: HashMap<String, ValueAndType>) -> RibInput {
        RibInput { input }
    }

    pub fn merge(&self, other: RibInput) -> RibInput {
        let mut cloned = self.clone();
        cloned.input.extend(other.input);
        cloned
    }
}
