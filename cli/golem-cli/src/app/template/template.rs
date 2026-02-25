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

use crate::app::template::generator::{
    generate_commons_by_template, generate_component_by_template,
    generate_on_demand_commons_by_template,
};
use crate::app::template::metadata::AppTemplateMetadata;
use crate::model::GuestLanguage;
use crate::{fs, SdkOverrides};
use golem_common::base_model::application::ApplicationName;
use golem_common::base_model::component::ComponentName;
use serde_derive::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Formatter;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AppTemplateName(String);

impl AppTemplateName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for AppTemplateName {
    fn from(s: &str) -> Self {
        AppTemplateName(s.to_string())
    }
}

impl From<String> for AppTemplateName {
    fn from(s: String) -> Self {
        AppTemplateName(s)
    }
}

impl fmt::Display for AppTemplateName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone)]
pub struct AppTemplate {
    pub name: AppTemplateName,
    pub language: GuestLanguage,
    pub template_path: PathBuf,
    pub metadata: AppTemplateMetadata,
    pub content_hash: Option<String>,
}

impl AppTemplate {
    pub fn load(language: GuestLanguage, template_path: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            name: fs::file_name_to_str(template_path)?.into(),
            language,
            template_path: template_path.into(),
            metadata: AppTemplateMetadata::load(template_path)?,
            content_hash: None,
        })
    }

    pub fn dev_only(&self) -> bool {
        (*match &self.metadata {
            AppTemplateMetadata::Common { dev_only, .. } => dev_only,
            AppTemplateMetadata::CommonOnDemand { dev_only, .. } => dev_only,
            AppTemplateMetadata::Component { dev_only, .. } => dev_only,
        })
        .unwrap_or(false)
    }

    pub fn description(&self) -> &str {
        match &self.metadata {
            AppTemplateMetadata::Common { description, .. } => description.as_deref().unwrap_or(""),
            AppTemplateMetadata::CommonOnDemand { description, .. } => {
                description.as_deref().unwrap_or("")
            }
            AppTemplateMetadata::Component { description, .. } => description.as_str(),
        }
    }

    pub fn skip_if_exists(&self) -> Option<&Path> {
        match &self.metadata {
            AppTemplateMetadata::Common { skip_if_exists, .. } => skip_if_exists.as_deref(),
            AppTemplateMetadata::CommonOnDemand { .. } => None,
            AppTemplateMetadata::Component { .. } => None,
        }
    }

    fn generate_commons(
        &self,
        application_name: &ApplicationName,
        target_path: &Path,
        sdk_overrides: &SdkOverrides,
    ) -> anyhow::Result<()> {
        generate_commons_by_template(self, application_name, target_path, sdk_overrides)
    }

    fn generate_on_demand_commons(
        &self,
        target_path: &Path,
        sdk_overrides: &SdkOverrides,
    ) -> anyhow::Result<()> {
        generate_on_demand_commons_by_template(self, target_path, sdk_overrides)
    }

    fn generate_component(
        &self,
        target_path: &Path,
        application_name: &ApplicationName,
        component_name: &ComponentName,
        sdk_overrides: &SdkOverrides,
    ) -> anyhow::Result<()> {
        generate_component_by_template(
            self,
            target_path,
            application_name,
            component_name,
            sdk_overrides,
        )
    }
}

#[derive(Debug, Clone)]
pub struct AppTemplateCommon(pub AppTemplate);

impl AppTemplateCommon {
    pub fn generate(
        &self,
        application_name: &ApplicationName,
        target_path: &Path,
        sdk_overrides: &SdkOverrides,
    ) -> anyhow::Result<()> {
        self.0
            .generate_commons(application_name, target_path, sdk_overrides)
    }
}

#[derive(Debug, Clone)]
pub struct AppTemplateCommonOnDemand(pub AppTemplate);

impl AppTemplateCommonOnDemand {
    pub fn generate(&self, target_path: &Path, sdk_overrides: &SdkOverrides) -> anyhow::Result<()> {
        self.0
            .generate_on_demand_commons(target_path, sdk_overrides)
    }
}

#[derive(Debug, Clone)]
pub struct AppTemplateComponent(pub AppTemplate);

impl AppTemplateComponent {
    pub fn generate(
        &self,
        application_name: &ApplicationName,
        component_name: &ComponentName,
        target_path: &Path,
        sdk_overrides: &SdkOverrides,
    ) -> anyhow::Result<()> {
        self.0
            .generate_component(target_path, application_name, component_name, sdk_overrides)
    }
}

#[derive(Debug, Default, Clone)]
pub struct AppTemplatesForLanguage {
    pub common: Option<AppTemplateCommon>,
    pub common_on_demand: Option<AppTemplateCommonOnDemand>,
    pub component: BTreeMap<AppTemplateName, AppTemplateComponent>,
}
