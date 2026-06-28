// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! Echo agent exercising the capability `Secret` schema type end-to-end.
//! The `QuotaToken` capability is exercised separately via the
//! `quota_rpc` agents.

use golem_rust::secrets::GuestSecretHandle;
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait CapabilityEchoAgent {
    fn new(name: String) -> Self;

    fn echo_secret(&self, value: GuestSecretHandle) -> GuestSecretHandle;
}

pub struct CapabilityEchoAgentImpl {
    _name: String,
}

#[agent_implementation]
impl CapabilityEchoAgent for CapabilityEchoAgentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn echo_secret(&self, value: GuestSecretHandle) -> GuestSecretHandle {
        value
    }
}
