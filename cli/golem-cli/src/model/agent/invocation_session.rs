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

use crate::model::cli_output::StructuredOutput;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInvocationSessionEvent {
    pub kind: AgentInvocationSessionEventKind,
    pub idempotency_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_stream_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
}

impl StructuredOutput for AgentInvocationSessionEvent {
    const KIND: &'static str = "agent.invoke-session";
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentInvocationSessionEventKind {
    Accepted,
    Rejected,
    Result,
    Item,
    End,
    StreamError,
    StreamCancel,
    Finished,
}

impl AgentInvocationSessionEvent {
    pub fn new(kind: AgentInvocationSessionEventKind, idempotency_key: impl Into<String>) -> Self {
        Self {
            kind,
            idempotency_key: idempotency_key.into(),
            agent_id: None,
            component_revision: None,
            outcome: None,
            reason: None,
            error: None,
            stream_id: None,
            parent_stream_id: None,
            path: None,
            offset: None,
            value: None,
        }
    }
}
