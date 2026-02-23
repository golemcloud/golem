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

use crate::app_template::{GOLEM_RUST_VERSION, GOLEM_TS_VERSION};
use crate::fs;
use crate::model::GuestLanguage;
use anyhow::anyhow;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::ComponentName;
use serde::{Deserialize, Serialize};
use std::fmt::Formatter;
use std::path::{Path, PathBuf};
use std::{fmt, io};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TemplateKind {
    Standalone,
    ComposableAppCommon {
        group: ComposableAppGroupName,
        skip_if_exists: Option<PathBuf>,
    },
    ComposableAppComponent {
        group: ComposableAppGroupName,
    },
}

impl TemplateKind {
    pub fn is_common(&self) -> bool {
        matches!(self, TemplateKind::ComposableAppCommon { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TemplateName(String);

impl TemplateName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for TemplateName {
    fn from(s: &str) -> Self {
        TemplateName(s.to_string())
    }
}

impl From<String> for TemplateName {
    fn from(s: String) -> Self {
        TemplateName(s)
    }
}

impl fmt::Display for TemplateName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ComposableAppGroupName(String);

impl ComposableAppGroupName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ComposableAppGroupName {
    fn default() -> Self {
        ComposableAppGroupName("default".to_string())
    }
}

impl From<&str> for ComposableAppGroupName {
    fn from(s: &str) -> Self {
        ComposableAppGroupName(s.to_string())
    }
}

impl From<String> for ComposableAppGroupName {
    fn from(s: String) -> Self {
        ComposableAppGroupName(s)
    }
}

impl fmt::Display for ComposableAppGroupName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Copy, Clone)]
pub enum TargetExistsResolveMode {
    Skip,
    MergeOrSkip,
    Fail,
    MergeOrFail,
}

pub type MergeContents = Box<dyn FnOnce(&[u8]) -> io::Result<Vec<u8>>>;

pub enum TargetExistsResolveDecision {
    Skip,
    Merge(MergeContents),
}

pub struct DocDependencyGroup {
    pub name: &'static str,
    pub dependencies: Vec<DocDependency>,
}

pub struct DocDependency {
    pub name: &'static str,
    pub env_vars: Vec<DocDependencyEnvVar>,
    pub url: String,
}

pub struct DocDependencyEnvVar {
    pub name: &'static str,
    pub value: &'static str,
    pub comment: &'static str,
}

#[derive(Debug, Clone)]
pub struct Template {
    pub name: TemplateName,
    pub kind: TemplateKind,
    pub language: GuestLanguage,
    pub description: String,
    pub template_path: PathBuf,
    pub instructions: String,
    pub dev_only: bool,
}

#[derive(Debug, Clone)]
pub struct TemplateParameters {
    pub application_name: ApplicationName,
    pub component_name: ComponentName,
    pub target_path: PathBuf,
    pub sdk_overrides: SdkOverrides,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TemplateMetadata {
    pub description: String,
    #[serde(rename = "appCommonGroup")]
    pub app_common_group: Option<String>,
    #[serde(rename = "appCommonSkipIfExists")]
    pub app_common_skip_if_exists: Option<String>,
    #[serde(rename = "appComponentGroup")]
    pub app_component_group: Option<String>,
    #[serde(rename = "requiresGolemHostWIT")]
    pub requires_golem_host_wit: Option<bool>,
    #[serde(rename = "requiresWASI")]
    pub requires_wasi: Option<bool>,
    pub exclude: Option<Vec<String>>,
    pub instructions: Option<String>,
    #[serde(rename = "devOnly")]
    pub dev_only: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Transform {
    PackageAndComponent,
    ManifestHints,
    TsSdk,
    RustSdk,
    ApplicationName,
}

#[derive(Debug, Clone, Default)]
pub struct SdkOverrides {
    pub golem_rust_path: Option<String>,
    pub golem_rust_version: Option<String>,
    pub ts_packages_path: Option<String>,
    pub ts_version: Option<String>,
}

impl SdkOverrides {
    pub fn ts_package_dep(&self, package_name: &str) -> String {
        match &self.ts_packages_path {
            Some(ts_packages_path) => {
                format!("{}/{}", ts_packages_path, package_name)
            }
            None => self
                .ts_version
                .as_deref()
                .unwrap_or(GOLEM_TS_VERSION)
                .to_string(),
        }
    }

    pub fn golem_rust_dep(&self) -> String {
        match &self.golem_rust_path {
            Some(rust_path) => {
                format!(r#"path = "{}""#, rust_path)
            }
            _ => {
                format!(
                    r#"version = "{}""#,
                    self.golem_rust_version
                        .as_deref()
                        .unwrap_or(GOLEM_RUST_VERSION)
                )
            }
        }
    }

    pub fn golem_client_dep(&self) -> anyhow::Result<String> {
        if let Some(rust_path) = &self.golem_rust_path {
            return Ok(format!(
                r#"path = "{}/golem-client""#,
                Self::golem_repo_path_from_golem_rust_path(rust_path)?
            ));
        }

        todo!("No published version yet")
    }

    pub fn golem_repo_path_from_golem_rust_path(path: &str) -> anyhow::Result<String> {
        let suffix = Path::new("sdks/rust/golem-rust");
        let path = Path::new(path);
        fs::path_to_str(path)?
            .strip_suffix(fs::path_to_str(suffix)?)
            .ok_or_else(|| anyhow!("Invalid Golem Rust path: {}", path.display()))
            .map(|s| s.to_string())
    }
}
