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
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct AgentDeleteResult {
    pub deleted: bool,
    pub agent: String,
}

impl TextView for AgentDeleteResult {
    fn log(&self) {}
}

impl CliOutput for AgentDeleteResult {
    const KIND: &'static str = "agent.delete.result";
}

#[derive(Debug, Clone, Serialize)]
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
    const KIND: &'static str = "agent.revert.result";
}

#[derive(Debug, Clone, Serialize)]
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
    const KIND: &'static str = "agent.plugin-toggle.result";
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanResult {
    pub cleaned: bool,
}

impl TextView for CleanResult {
    fn log(&self) {}
}

impl CliOutput for CleanResult {
    const KIND: &'static str = "app.clean.result";
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildResult {
    pub built: bool,
}

impl TextView for BuildResult {
    fn log(&self) {}
}

impl CliOutput for BuildResult {
    const KIND: &'static str = "app.build.result";
}

#[derive(Debug, Clone, Serialize)]
pub struct NewAppResult {
    pub created: bool,
    pub application_name: String,
    pub application_dir: PathBuf,
}

impl TextView for NewAppResult {
    fn log(&self) {}
}

impl CliOutput for NewAppResult {
    const KIND: &'static str = "app.new.result";
}

#[derive(Debug, Clone, Serialize)]
pub struct DeployResultView {
    pub deployed: bool,
}

impl TextView for DeployResultView {
    fn log(&self) {}
}

impl CliOutput for DeployResultView {
    const KIND: &'static str = "app.deploy.result";
}

#[derive(Debug, Clone, Serialize)]
pub struct GenerateBridgeResult {
    pub generated: bool,
}

impl TextView for GenerateBridgeResult {
    fn log(&self) {}
}

impl CliOutput for GenerateBridgeResult {
    const KIND: &'static str = "app.generate-bridge.result";
}
