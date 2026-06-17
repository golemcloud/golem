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
//! Each view's `TextView::log` is a no-op: when `--format text` is used
//! (the default), the user has already seen the progress lines on stdout
//! and adding another rendering of the same information would just be
//! noise. When `--format json/yaml/toon` is used, the progress text is routed
//! to stderr (see `Context::new`) and these structured payloads are
//! emitted on stdout so that automation can rely on a stable schema.

use crate::model::cli_output::CliOutput;
use crate::model::text::fmt::TextView;
use golem_common::model::component::ComponentName;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDeleteResult {
    pub deleted: bool,
    pub agent: String,
}

impl TextView for AgentDeleteResult {
    fn log(&self) {}
}

impl CliOutput for AgentDeleteResult {
    const KIND: &'static str = "agent.delete";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileContentsResult {
    pub saved: bool,
    pub agent: String,
    pub path: String,
    pub output_path: PathBuf,
    pub bytes: usize,
}

impl TextView for AgentFileContentsResult {
    fn log(&self) {}
}

impl CliOutput for AgentFileContentsResult {
    const KIND: &'static str = "agent.file-contents";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInterruptResult {
    pub interrupted: bool,
    pub agent: String,
}

impl TextView for AgentInterruptResult {
    fn log(&self) {}
}

impl CliOutput for AgentInterruptResult {
    const KIND: &'static str = "agent.interrupt";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentResumeResult {
    pub resumed: bool,
    pub agent: String,
}

impl TextView for AgentResumeResult {
    fn log(&self) {}
}

impl CliOutput for AgentResumeResult {
    const KIND: &'static str = "agent.resume";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSimulateCrashResult {
    pub simulated: bool,
    pub agent: String,
}

impl TextView for AgentSimulateCrashResult {
    fn log(&self) {}
}

impl CliOutput for AgentSimulateCrashResult {
    const KIND: &'static str = "agent.simulate-crash";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRevertResult {
    pub reverted: bool,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_oplog_index: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_of_invocations: Option<u64>,
}

impl TextView for AgentRevertResult {
    fn log(&self) {}
}

impl CliOutput for AgentRevertResult {
    const KIND: &'static str = "agent.revert";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCancelInvocationResult {
    pub canceled: bool,
    pub agent: String,
    pub idempotency_key: String,
}

impl TextView for AgentCancelInvocationResult {
    fn log(&self) {}
}

impl CliOutput for AgentCancelInvocationResult {
    const KIND: &'static str = "agent.cancel-invocation";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRedeployResult {
    pub redeployed: bool,
    pub components: Vec<ComponentName>,
}

impl TextView for AgentRedeployResult {
    fn log(&self) {}
}

impl CliOutput for AgentRedeployResult {
    const KIND: &'static str = "agent.redeploy";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPluginToggleResult {
    pub activated: bool,
    pub agent: String,
    pub plugin: String,
    pub priority: i32,
}

impl TextView for AgentPluginToggleResult {
    fn log(&self) {}
}

impl CliOutput for AgentPluginToggleResult {
    const KIND: &'static str = "agent.plugin-toggle";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanResult {
    pub cleaned: bool,
}

impl TextView for CleanResult {
    fn log(&self) {}
}

impl CliOutput for CleanResult {
    const KIND: &'static str = "clean";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildResult {
    pub built: bool,
}

impl TextView for BuildResult {
    fn log(&self) {}
}

impl CliOutput for BuildResult {
    const KIND: &'static str = "build";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAppResult {
    pub created: bool,
    pub application_name: String,
    pub application_dir: PathBuf,
}

impl TextView for NewAppResult {
    fn log(&self) {}
}

impl CliOutput for NewAppResult {
    const KIND: &'static str = "new";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployResultView {
    pub deployed: bool,
}

impl TextView for DeployResultView {
    fn log(&self) {}
}

impl CliOutput for DeployResultView {
    const KIND: &'static str = "deploy";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateBridgeResult {
    pub generated: bool,
}

impl TextView for GenerateBridgeResult {
    fn log(&self) {}
}

impl CliOutput for GenerateBridgeResult {
    const KIND: &'static str = "generate-bridge";
}
