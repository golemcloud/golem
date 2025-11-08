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

use std::cell::RefCell;

use crate::{agentic::Agent, golem_agentic::golem::api::host::AgentId};
use wasi_async_runtime::{block_on, Reactor};

pub struct ResolvedAgent {
    pub agent: RefCell<Box<dyn Agent>>,
    pub agent_id: AgentId,
    pub reactor: Reactor,
}

impl ResolvedAgent {
    pub fn new(agent: Box<dyn Agent>, agent_id: AgentId) -> ResolvedAgent {
        block_on(|reactor| async move {
            ResolvedAgent {
                agent: RefCell::new(agent),
                agent_id,
                reactor,
            }
        })
    }
}
