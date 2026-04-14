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

use crate::app::template::AppTemplateName;
use crate::app::template::metadata::AppTemplateMetadata;
use crate::app::template::template::{
    AppTemplate, AppTemplateAgent, AppTemplateCommon, AppTemplateCommonOnDemand,
    AppTemplateComponent, AppTemplatesForLanguage,
};
use crate::fs;
use crate::model::GuestLanguage;
use anyhow::{Context, anyhow, bail};
use include_dir::{Dir, include_dir};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub static TEMPLATES_DIR: Dir<'static> = include_dir!("$OUT_DIR/templates");

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

    pub fn common_template(
        &self,
        language: GuestLanguage,
    ) -> anyhow::Result<&Option<AppTemplateCommon>> {
        Ok(&self.language_templates(language)?.common)
    }

    pub fn common_template_file_contents(
        &self,
        language: GuestLanguage,
        relative_path: &Path,
    ) -> anyhow::Result<Option<String>> {
        let Some(common_template) = self.common_template(language)?.as_ref() else {
            return Ok(None);
        };

        Self::template_file_contents(&common_template.0, relative_path)
    }

    pub fn common_on_demand_template(
        &self,
        language: GuestLanguage,
    ) -> anyhow::Result<&Option<AppTemplateCommonOnDemand>> {
        Ok(&self.language_templates(language)?.common_on_demand)
    }

    pub fn component_templates(
        &self,
        language: GuestLanguage,
    ) -> anyhow::Result<&Option<AppTemplateComponent>> {
        Ok(&self.language_templates(language)?.component)
    }

    pub fn component_template(
        &self,
        language: GuestLanguage,
    ) -> anyhow::Result<&Option<AppTemplateComponent>> {
        Ok(&self.language_templates(language)?.component)
    }

    pub fn agent_templates(
        &self,
        language: GuestLanguage,
    ) -> anyhow::Result<&BTreeMap<AppTemplateName, AppTemplateAgent>> {
        Ok(&self.language_templates(language)?.agent)
    }

    pub fn agent_template(
        &self,
        template_name: &AppTemplateName,
    ) -> anyhow::Result<&AppTemplateAgent> {
        self.language_templates(template_name.language())?
            .agent
            .get(template_name)
            .ok_or_else(|| {
                anyhow!(
                    "{} template '{}' not found",
                    template_name.language(),
                    template_name
                )
            })
    }

    pub fn search_agent_templates(
        &self,
        language: Option<GuestLanguage>,
        query: Option<&str>,
    ) -> BTreeMap<GuestLanguage, BTreeMap<&AppTemplateName, &AppTemplateAgent>> {
        let query = query.map(|q| q.to_lowercase());
        let query = query.as_ref();

        self.templates
            .iter()
            .filter(|&(&lang, _)| language.is_none_or(|l| lang == l))
            .map(|(lang, lang_templates)| {
                (
                    *lang,
                    lang_templates
                        .agent
                        .iter()
                        .filter(|(name, template)| {
                            query.is_none_or(|q| {
                                name.name().to_lowercase().contains(q)
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
                    if entry.common.is_some() {
                        bail!(
                            "Multiple common templates found for {}",
                            template.language.name()
                        );
                    }
                    entry.common = Some(AppTemplateCommon(template));
                }
                AppTemplateMetadata::CommonOnDemand { .. } => {
                    if entry.common_on_demand.is_some() {
                        bail!(
                            "Multiple common on-demand templates found for {}",
                            template.language.name()
                        );
                    }
                    entry.common_on_demand = Some(AppTemplateCommonOnDemand(template));
                }
                AppTemplateMetadata::Component { .. } => {
                    if entry.component.is_some() {
                        bail!(
                            "Multiple component templates found for {}",
                            template.language.name()
                        );
                    }
                    entry.component = Some(AppTemplateComponent(template));
                }
                AppTemplateMetadata::Agent { .. } => {
                    entry
                        .agent
                        .insert(template.name.clone(), AppTemplateAgent(template));
                }
            }
        }

        Ok(templates)
    }

    pub fn common_template_skill_files(
        &self,
        language: GuestLanguage,
    ) -> anyhow::Result<Vec<(PathBuf, String)>> {
        let Some(common_template) = self.common_template(language)?.as_ref() else {
            return Ok(Vec::new());
        };

        let skills_path = common_template.0.template_path.join(".agents/skills");
        let Some(skills_dir) = TEMPLATES_DIR.get_dir(&skills_path) else {
            return Ok(Vec::new());
        };

        let mut result = Vec::new();
        Self::collect_skill_files(skills_dir, &common_template.0.template_path, &mut result)?;
        Ok(result)
    }

    fn collect_skill_files(
        dir: &Dir,
        template_root: &Path,
        result: &mut Vec<(PathBuf, String)>,
    ) -> anyhow::Result<()> {
        for entry in dir.entries() {
            if let Some(sub_dir) = entry.as_dir() {
                Self::collect_skill_files(sub_dir, template_root, result)?;
            } else if let Some(file) = entry.as_file()
                && fs::file_name_to_str(file.path())? == "SKILL.md"
            {
                let relative_path = fs::strip_prefix_or_err(file.path(), template_root)?;
                let contents = file
                    .contents_utf8()
                    .ok_or_else(|| {
                        anyhow!("Skill file is not valid UTF-8: {}", file.path().display())
                    })?
                    .to_string();
                result.push((relative_path.to_path_buf(), contents));
            }
        }
        Ok(())
    }

    fn template_file_contents(
        template: &AppTemplate,
        relative_path: &Path,
    ) -> anyhow::Result<Option<String>> {
        let file_path = template.template_path.join(relative_path);
        let Some(file) = TEMPLATES_DIR.get_file(file_path.as_path()) else {
            return Ok(None);
        };

        let source = file
            .contents_utf8()
            .ok_or_else(|| anyhow!("Template file is not valid UTF-8: {}", file_path.display()))?;

        Ok(Some(source.to_string()))
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
                    let mut template = AppTemplate::load(lang, template_dir.path())?;
                    if template.metadata.is_common_on_demand() {
                        template.content_hash = Some(Self::hash_template_dir(template_dir)?);
                    }
                    if dev_mode || !template.dev_only() {
                        result.push(template);
                    }
                }
            }
        }
        Ok(result)
    }

    fn hash_template_dir(template_dir: &Dir) -> anyhow::Result<String> {
        let mut file_hashes_by_path = BTreeMap::<String, String>::new();
        let root = template_dir.path();
        Self::collect_file_hashes(template_dir, root, &mut file_hashes_by_path)?;
        let serialized = serde_json::to_vec(&file_hashes_by_path)
            .context("Failed to serialize on-demand template file hash map")?;
        Ok(blake3::hash(&serialized).to_hex().to_string())
    }

    fn collect_file_hashes(
        dir: &Dir,
        root: &Path,
        file_hashes: &mut BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        for entry in dir.entries() {
            if let Some(sub_dir) = entry.as_dir() {
                Self::collect_file_hashes(sub_dir, root, file_hashes)?;
            } else if let Some(file) = entry.as_file() {
                let relative_path = fs::strip_prefix_or_err(file.path(), root)?;
                let mut relative_path_str = fs::path_to_str(relative_path)?.to_string();
                if std::path::MAIN_SEPARATOR != '/' {
                    relative_path_str = relative_path_str.replace('\\', "/");
                }
                let hash = blake3::hash(file.contents()).to_hex().to_string();
                file_hashes.insert(relative_path_str, hash);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::AppTemplateRepo;
    use crate::model::GuestLanguage;
    use std::fs as stdfs;
    use std::path::{Path, PathBuf};
    use test_r::test;

    fn canonical_skill_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../golem-skills/skills/common/golem-new-project/SKILL.md")
    }

    #[test]
    fn embedded_common_bootstrap_skill_matches_canonical_source() {
        let repo = AppTemplateRepo::get(false).unwrap();
        let canonical = stdfs::read_to_string(canonical_skill_path()).unwrap();

        let relative_path = Path::new(".agents/skills/golem-new-project/SKILL.md");

        for language in [
            GuestLanguage::TypeScript,
            GuestLanguage::Rust,
            GuestLanguage::Scala,
        ] {
            let embedded = repo
                .common_template_file_contents(language, relative_path)
                .unwrap()
                .unwrap();

            assert_eq!(
                embedded,
                canonical,
                "{} common template skill drifted for {}",
                language.name(),
                relative_path.display(),
            );
        }
    }
}
