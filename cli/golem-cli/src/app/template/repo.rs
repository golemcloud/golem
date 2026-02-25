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

use crate::app::template::metadata::AppTemplateMetadata;
use crate::app::template::template::{
    AppTemplate, AppTemplateCommon, AppTemplateCommonOnDemand, AppTemplateComponent,
    AppTemplatesForLanguage,
};
use crate::app::template::AppTemplateName;
use crate::fs;
use crate::model::GuestLanguage;
use anyhow::{anyhow, bail};
use include_dir::{include_dir, Dir};
use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

pub static TEMPLATES_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/templates");

pub type GroupedAppTemplates = BTreeMap<GuestLanguage, AppTemplatesForLanguage>;

static TEMPLATE_REPO: LazyLock<anyhow::Result<AppTemplateRepo>> =
    LazyLock::new(|| AppTemplateRepo::new(false));
static TEMPLATES_REPO_DEV_MODE: LazyLock<anyhow::Result<AppTemplateRepo>> =
    LazyLock::new(|| AppTemplateRepo::new(true));

#[derive(Debug)]
pub struct AppTemplateRepo {
    languages: HashSet<GuestLanguage>,
    templates: GroupedAppTemplates,
}

impl AppTemplateRepo {
    fn new(dev_mode: bool) -> anyhow::Result<Self> {
        let templates = Self::collect_grouped_templates(dev_mode)?;
        Ok(Self {
            languages: templates.keys().cloned().collect::<HashSet<_>>(),
            templates,
        })
    }

    pub fn get(dev_mode: bool) -> anyhow::Result<&'static AppTemplateRepo> {
        if dev_mode {
            TEMPLATES_REPO_DEV_MODE.as_ref().map_err(|err| anyhow!(err))
        } else {
            TEMPLATE_REPO.as_ref().map_err(|err| anyhow!(err))
        }
    }

    pub fn languages(&self) -> &HashSet<GuestLanguage> {
        &self.languages
    }

    pub fn common_templates(
        &self,
        language: GuestLanguage,
    ) -> anyhow::Result<&BTreeMap<AppTemplateName, AppTemplateCommon>> {
        Ok(&self.language_templates(language)?.common)
    }

    pub fn common_on_demand_templates(
        &self,
        language: GuestLanguage,
    ) -> anyhow::Result<&BTreeMap<AppTemplateName, AppTemplateCommonOnDemand>> {
        Ok(&self.language_templates(language)?.common_on_demand)
    }

    pub fn component_templates(
        &self,
        language: GuestLanguage,
    ) -> anyhow::Result<&BTreeMap<AppTemplateName, AppTemplateComponent>> {
        Ok(&self.language_templates(language)?.component)
    }

    pub fn component_template(
        &self,
        language: GuestLanguage,
        template_name: &AppTemplateName,
    ) -> anyhow::Result<&AppTemplateComponent> {
        self.language_templates(language)?
            .component
            .get(template_name)
            .ok_or_else(|| anyhow!("{} template '{}' not found", language, template_name))
    }

    pub fn search_component_templates(
        &self,
        language: Option<GuestLanguage>,
        query: Option<&str>,
    ) -> BTreeMap<GuestLanguage, BTreeMap<&AppTemplateName, &AppTemplateComponent>> {
        let query = query.map(|q| q.to_lowercase());
        let query = query.as_ref();

        self.templates
            .iter()
            .filter(|(&lang, _)| language.is_none_or(|l| lang == l))
            .map(|(lang, lang_templates)| {
                (
                    *lang,
                    lang_templates
                        .component
                        .iter()
                        .filter(|(name, template)| {
                            query.is_none_or(|q| {
                                name.as_str().to_lowercase().contains(q)
                                    || template.0.description().to_lowercase().contains(q)
                            })
                        })
                        .collect(),
                )
            })
            .collect()
    }

    fn language_templates(
        &self,
        language: GuestLanguage,
    ) -> anyhow::Result<&AppTemplatesForLanguage> {
        self.templates
            .get(&language)
            .ok_or_else(|| anyhow!("No templates are available for {}", language))
    }

    fn collect_grouped_templates(dev_mode: bool) -> anyhow::Result<GroupedAppTemplates> {
        let mut templates = BTreeMap::<GuestLanguage, AppTemplatesForLanguage>::new();

        for template in Self::collect_templates(dev_mode)? {
            let entry = templates.entry(template.language).or_default();
            match &template.metadata {
                AppTemplateMetadata::Common { .. } => {
                    entry
                        .common
                        .insert(template.name.clone(), AppTemplateCommon(template));
                }
                AppTemplateMetadata::CommonOnDemand { .. } => {
                    entry
                        .common_on_demand
                        .insert(template.name.clone(), AppTemplateCommonOnDemand(template));
                }
                AppTemplateMetadata::Component { .. } => {
                    entry
                        .component
                        .insert(template.name.clone(), AppTemplateComponent(template));
                }
            }
        }

        Ok(templates)
    }

    fn collect_templates(dev_mode: bool) -> anyhow::Result<Vec<AppTemplate>> {
        let mut result = vec![];
        for entry in TEMPLATES_DIR.entries() {
            let Some(lang_dir) = entry.as_dir() else {
                continue;
            };

            let lang_dir_name = fs::file_name_to_str(lang_dir.path())?;

            let Some(lang) = GuestLanguage::from_id_string(lang_dir_name) else {
                bail!(
                    "Invalid guest language template directory: {}",
                    lang_dir.path().display()
                );
            };

            for sub_entry in lang_dir.entries() {
                if let Some(template_dir) = sub_entry.as_dir() {
                    let template = AppTemplate::load(lang, template_dir.path())?;
                    if dev_mode || !template.dev_only() {
                        result.push(template);
                    }
                }
            }
        }
        Ok(result)
    }
}
