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

use crate::app::build::check::DependencyFixStep;
use crate::app::context::BuildContext;
use crate::app::edit;
use crate::app::template::AppTemplateRepo;
use crate::fs;
use crate::model::GuestLanguage;
use anyhow::anyhow;
use std::collections::BTreeSet;
use std::path::Path;

pub(super) fn plan_agents_md_fix_step(
    ctx: &BuildContext<'_>,
    selected_languages: &BTreeSet<GuestLanguage>,
) -> anyhow::Result<Option<DependencyFixStep>> {
    if selected_languages.is_empty() {
        return Ok(None);
    }

    let app_template_repo = AppTemplateRepo::get(ctx.application_config().dev_mode)?;
    let agents_md_path = ctx.application().app_root_dir().join("AGENTS.md");
    let current = if agents_md_path.exists() {
        fs::read_to_string(&agents_md_path)?
    } else {
        String::new()
    };

    let mut template_guides = Vec::new();
    for language in selected_languages {
        let key = language.id().to_string();
        let template_source = app_template_repo
            .common_template_file_contents(*language, Path::new("AGENTS.md"))?
            .ok_or_else(|| {
                anyhow!(
                    "Could not find AGENTS.md in {} common template",
                    language.name()
                )
            })?;

        let guide = edit::agents_md::extract_managed_guide(template_source.as_str(), key.as_str())
            .ok_or_else(|| {
                anyhow!(
                    "Missing '{}' managed section in {} common template AGENTS.md",
                    key,
                    language.name()
                )
            })?;

        template_guides.push((key, guide));
    }

    let needs_update = template_guides
        .iter()
        .any(|(key, expected)| managed_guide_differs(current.as_str(), key, expected));

    if !needs_update {
        return Ok(None);
    }

    let replacement = template_guides
        .iter()
        .map(|(key, content)| edit::agents_md::render_managed_guide(key, content))
        .collect::<Vec<_>>()
        .join("\n");
    let new = edit::agents_md::merge_guides(current.as_str(), replacement.as_str())?;

    if new == current {
        Ok(None)
    } else {
        Ok(Some(DependencyFixStep {
            path: agents_md_path,
            current,
            new,
        }))
    }
}

fn managed_guide_differs(current_source: &str, key: &str, expected_content: &str) -> bool {
    edit::agents_md::extract_managed_guide(current_source, key).as_deref() != Some(expected_content)
}

#[cfg(test)]
mod tests {
    use super::managed_guide_differs;
    use crate::app::edit::agents_md;
    use crate::app::template::AppTemplateRepo;
    use crate::model::GuestLanguage;
    use std::path::Path;
    use test_r::test;

    #[test]
    fn managed_guide_compare_ignores_wrapper_newlines() {
        let current = agents_md::render_managed_guide("rust", "# Rust guide\nBody\n");

        assert!(!managed_guide_differs(
            &current,
            "rust",
            "# Rust guide\nBody"
        ));
    }

    #[test]
    fn language_common_templates_include_expected_agents_sections() {
        let repo = AppTemplateRepo::get(false).unwrap();

        for language in [GuestLanguage::TypeScript, GuestLanguage::Rust] {
            let source = repo
                .common_template_file_contents(language, Path::new("AGENTS.md"))
                .unwrap()
                .unwrap();

            assert!(
                agents_md::extract_managed_guide(source.as_str(), language.id()).is_some(),
                "missing managed section '{}' in {} common template AGENTS.md",
                language.id(),
                language.name(),
            );
        }
    }
}
