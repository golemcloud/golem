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

//! Lightweight structured result views for commands whose human-readable
//! output is mostly progress text printed during the run.
//!
//! Each view implements `NoTextOutput`: when `--format text` is used
//! (the default), the user has already seen the progress lines on stdout
//! and adding another rendering of the same information would just be
//! noise. When `--format json/yaml/toon` is used, the progress text is routed
//! to stderr (see `Context::new`) and these structured payloads are
//! emitted on stdout so that automation can rely on a stable schema.

use crate::model::agent::RawAgentId;
use crate::model::cli_output::StructuredOutput;
use crate::model::masking::Masked;
use crate::model::text_format::*;
use golem_common::model::component::{ComponentName, ComponentRevision};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDeleteView {
    pub deleted: bool,
    pub agent_id: String,
}

impl Masked for AgentDeleteView {}

impl MessageWithFields for AgentDeleteView {
    fn message(&self) -> String {
        format!("Deleted agent {}", format_message_highlight(&self.agent_id))
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();
        fields.fmt_field("Agent ID", &self.agent_id, format_main_id);
        fields.build()
    }
}

impl StructuredOutput for AgentDeleteView {
    const KIND: &'static str = "agent.delete";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileContentsResult {
    pub saved: bool,
    pub agent_id: String,
    pub path: String,
    pub output_path: PathBuf,
    pub bytes: usize,
}

impl NoTextOutput for AgentFileContentsResult {}
impl TextOutput for AgentFileContentsResult {}

impl StructuredOutput for AgentFileContentsResult {
    const KIND: &'static str = "agent.file-contents";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInterruptResult {
    pub interrupted: bool,
    pub agent_id: String,
}

impl NoTextOutput for AgentInterruptResult {}
impl TextOutput for AgentInterruptResult {}

impl StructuredOutput for AgentInterruptResult {
    const KIND: &'static str = "agent.interrupt";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentResumeResult {
    pub resumed: bool,
    pub agent_id: String,
}

impl NoTextOutput for AgentResumeResult {}
impl TextOutput for AgentResumeResult {}

impl StructuredOutput for AgentResumeResult {
    const KIND: &'static str = "agent.resume";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSimulateCrashResult {
    pub simulated: bool,
    pub agent_id: String,
}

impl NoTextOutput for AgentSimulateCrashResult {}
impl TextOutput for AgentSimulateCrashResult {}

impl StructuredOutput for AgentSimulateCrashResult {
    const KIND: &'static str = "agent.simulate-crash";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRevertResult {
    pub reverted: bool,
    pub agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_oplog_index: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_of_invocations: Option<u64>,
}

impl NoTextOutput for AgentRevertResult {}
impl TextOutput for AgentRevertResult {}

impl StructuredOutput for AgentRevertResult {
    const KIND: &'static str = "agent.revert";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCancelInvocationResult {
    pub canceled: bool,
    pub agent_id: String,
    pub idempotency_key: String,
}

impl NoTextOutput for AgentCancelInvocationResult {}
impl TextOutput for AgentCancelInvocationResult {}

impl StructuredOutput for AgentCancelInvocationResult {
    const KIND: &'static str = "agent.cancel-invocation";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRedeployResult {
    pub redeployed: bool,
    pub agents: Vec<AgentRedeploymentMeta>,
}

impl NoTextOutput for AgentRedeployResult {}
impl TextOutput for AgentRedeployResult {}

impl StructuredOutput for AgentRedeployResult {
    const KIND: &'static str = "agent.redeploy";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRedeploymentMeta {
    pub component_name: ComponentName,
    pub agent_id: RawAgentId,
    pub from_revision: ComponentRevision,
    pub revision: ComponentRevision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDeleteAllResult {
    pub deleted: bool,
    pub agents: Vec<AgentDeletionMeta>,
}

impl NoTextOutput for AgentDeleteAllResult {}
impl TextOutput for AgentDeleteAllResult {}

impl StructuredOutput for AgentDeleteAllResult {
    const KIND: &'static str = "agent.delete-all";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDeletionMeta {
    pub component_name: ComponentName,
    pub agent_id: RawAgentId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPluginToggleResult {
    pub activated: bool,
    pub agent_id: String,
    pub plugin: String,
    pub priority: i32,
}

impl NoTextOutput for AgentPluginToggleResult {}
impl TextOutput for AgentPluginToggleResult {}

impl StructuredOutput for AgentPluginToggleResult {
    const KIND: &'static str = "agent.plugin-toggle";
}
