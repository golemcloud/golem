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

use crate::model::cli_output::StructuredOutput;
use crate::model::text::fmt::{NoTextOutput, TextOutput};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanResult {
    pub cleaned: bool,
}

impl NoTextOutput for CleanResult {}
impl TextOutput for CleanResult {}

impl StructuredOutput for CleanResult {
    const KIND: &'static str = "clean";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildResult {
    pub built: bool,
}

impl NoTextOutput for BuildResult {}
impl TextOutput for BuildResult {}

impl StructuredOutput for BuildResult {
    const KIND: &'static str = "build";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAppResult {
    pub created: bool,
    pub application_name: String,
    pub application_dir: PathBuf,
}

impl NoTextOutput for NewAppResult {}
impl TextOutput for NewAppResult {}

impl StructuredOutput for NewAppResult {
    const KIND: &'static str = "new";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployResultView {
    pub deployed: bool,
}

impl NoTextOutput for DeployResultView {}
impl TextOutput for DeployResultView {}

impl StructuredOutput for DeployResultView {
    const KIND: &'static str = "deploy";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateBridgeResult {
    pub generated: bool,
}

impl NoTextOutput for GenerateBridgeResult {}
impl TextOutput for GenerateBridgeResult {}

impl StructuredOutput for GenerateBridgeResult {
    const KIND: &'static str = "generate-bridge";
}
