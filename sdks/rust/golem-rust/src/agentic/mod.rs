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

pub use agent::*;
pub use agent_arg_schema::*;
pub use agent_initiator::*;
pub use agent_registry::*;
pub use agent_type_name::*;

mod agent;
mod agent_arg_schema;
mod agent_impl;
mod agent_initiator;
mod agent_registry;
mod agent_type_name;

pub struct ResolvedAgent {
    pub agent: Box<dyn agent::Agent>,
}
