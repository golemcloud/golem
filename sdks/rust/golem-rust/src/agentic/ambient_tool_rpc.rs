// Copyright 2024-2026 Golem Cloud
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

use crate::bindings::golem::tool::host::ToolRpc;

/// SDK-owned ambient tool invocation backend used by generated typed clients.
///
/// Keeping the raw WIT resource behind this type prevents generated code from
/// depending on the bindings of a particular exported guest world.
pub struct AmbientToolRpc {
    pub(crate) inner: ToolRpc,
}

impl AmbientToolRpc {
    pub fn new(tool_name: &str) -> Self {
        Self {
            inner: ToolRpc::new(tool_name),
        }
    }
}
