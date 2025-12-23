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

use desert_rust::BinaryCodec;
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[derive(Debug, Hash, PartialEq, Eq, Clone, Ord, PartialOrd, BinaryCodec)]
#[desert(evolution())]
pub struct ComponentDependencyKey {
    pub component_name: String,
    pub component_id: Uuid,
    pub component_revision: u64,
    pub root_package_name: Option<String>,
    pub root_package_version: Option<String>,
}

impl Display for ComponentDependencyKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Component: {}, ID: {}, Revision: {}, Root Package: {}@{}",
            self.component_name,
            self.component_id,
            self.component_revision,
            self.root_package_name.as_deref().unwrap_or("unknown"),
            self.root_package_version.as_deref().unwrap_or("unknown")
        )
    }
}
