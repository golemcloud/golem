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

use crate::app::template::generator::{
    generate_agent_by_template, generate_commons_by_template, generate_component_by_template,
    generate_on_demand_commons_by_template, InMemoryFs, StdFs,
};
use crate::app::template::metadata::AppTemplateMetadata;
use crate::fs;
use crate::model::GuestLanguage;
use golem_common::base_model::application::ApplicationName;
use golem_common::base_model::component::ComponentName;
use serde_derive::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AppTemplateName {
    language: GuestLanguage,
    name: String,

    rendered_name: String,
}

impl AppTemplateName {
    pub fn new(language: GuestLanguage, name: String) -> Self {
        let rendered_name = {
            if name == "default" {
                language.id().to_string()
            } else {
                format!("{}/{}", language.id(), name)
            }
        };

        Self {
            language,
            name,
            rendered_name,
        }
    }

    pub fn language(&self) -> GuestLanguage {
        self.language
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn as_str(&self) -> &str {
        &self.rendered_name
    }
}

impl FromStr for AppTemplateName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some((lang, name)) = s.split_once("/") else {
            return match GuestLanguage::from_id_string(s) {
                Some(language) => Ok(Self::new(language, "default".to_string())),
                None => Err(format!("Missing language prefix in template name: {}", s)),
            };
        };

        let language = GuestLanguage::from_id_string(lang).ok_or_else(|| {
            format!(
                "Failed to parse template language prefix {} for template name: {}",
                lang, s
            )
        })?;

        Ok(Self::new(language, name.to_string()))
    }
}

impl Display for AppTemplateName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
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
            name: AppTemplateName::new(language, fs::file_name_to_str(template_path)?.to_string()),
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
            AppTemplateMetadata::Agent { dev_only, .. } => dev_only,
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
            AppTemplateMetadata::Agent { description, .. } => description.as_str(),
        }
    }

    fn generate_commons(
        &self,
        application_name: &ApplicationName,
        target_path: &Path,
    ) -> anyhow::Result<InMemoryFs> {
        generate_commons_by_template(self, application_name, target_path, InMemoryFs::new())
    }

    fn generate_on_demand_commons(
        &self,
        application_dir: &Path,
        target_path: &Path,
    ) -> anyhow::Result<()> {
        generate_on_demand_commons_by_template(self, application_dir, target_path, StdFs)
    }

    fn generate_component(
        &self,
        application_name: &ApplicationName,
        application_dir: &Path,
        component_name: &ComponentName,
        component_dir: &Path,
    ) -> anyhow::Result<InMemoryFs> {
        generate_component_by_template(
            self,
            application_name,
            application_dir,
            component_name,
            component_dir,
            InMemoryFs::new(),
        )
    }

    fn generate_agent(
        &self,
        application_name: &ApplicationName,
        application_dir: &Path,
        component_name: &ComponentName,
        component_dir: &Path,
    ) -> anyhow::Result<InMemoryFs> {
        generate_agent_by_template(
            self,
            application_name,
            application_dir,
            component_name,
            component_dir,
            InMemoryFs::new(),
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
    ) -> anyhow::Result<InMemoryFs> {
        self.0.generate_commons(application_name, target_path)
    }
}

#[derive(Debug, Clone)]
pub struct AppTemplateCommonOnDemand(pub AppTemplate);

impl AppTemplateCommonOnDemand {
    pub fn generate(&self, application_dir: &Path, target_path: &Path) -> anyhow::Result<()> {
        self.0
            .generate_on_demand_commons(application_dir, target_path)
    }
}

#[derive(Debug, Clone)]
pub struct AppTemplateComponent(pub AppTemplate);

impl AppTemplateComponent {
    pub fn generate(
        &self,
        application_name: &ApplicationName,
        application_dir: &Path,
        component_name: &ComponentName,
        component_dir: &Path,
    ) -> anyhow::Result<InMemoryFs> {
        self.0.generate_component(
            application_name,
            application_dir,
            component_name,
            component_dir,
        )
    }
}

#[derive(Debug, Clone)]
pub struct AppTemplateAgent(pub AppTemplate);

impl AppTemplateAgent {
    pub fn generate(
        &self,
        application_name: &ApplicationName,
        application_dir: &Path,
        component_name: &ComponentName,
        component_dir: &Path,
    ) -> anyhow::Result<InMemoryFs> {
        self.0.generate_agent(
            application_name,
            application_dir,
            component_name,
            component_dir,
        )
    }
}

#[derive(Debug, Default, Clone)]
pub struct AppTemplatesForLanguage {
    pub common: Option<AppTemplateCommon>,
    pub common_on_demand: Option<AppTemplateCommonOnDemand>,
    pub component: Option<AppTemplateComponent>,
    pub agent: BTreeMap<AppTemplateName, AppTemplateAgent>,
}
