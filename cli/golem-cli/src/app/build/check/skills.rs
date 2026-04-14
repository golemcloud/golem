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
use crate::app::template::AppTemplateRepo;
use crate::fs;
use crate::model::GuestLanguage;
use anyhow::bail;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub(super) fn plan_skill_fix_steps(
    ctx: &BuildContext<'_>,
    selected_languages: &BTreeSet<GuestLanguage>,
) -> anyhow::Result<Vec<DependencyFixStep>> {
    if selected_languages.is_empty() {
        return Ok(Vec::new());
    }

    let app_template_repo = AppTemplateRepo::get(ctx.application_config().dev_mode)?;
    let app_root = ctx.application().app_root_dir();

    let mut expected_files: BTreeMap<PathBuf, (GuestLanguage, String)> = BTreeMap::new();
    for &language in selected_languages {
        for (rel_path, content) in app_template_repo.common_template_skill_files(language)? {
            match expected_files.entry(rel_path.clone()) {
                Entry::Vacant(v) => {
                    v.insert((language, content));
                }
                Entry::Occupied(o) => {
                    let (prev_lang, prev_content) = o.get();
                    if *prev_content != content {
                        bail!(
                            "Conflicting embedded skill {} for {} and {}",
                            rel_path.display(),
                            prev_lang.name(),
                            language.name()
                        );
                    }
                }
            }
        }
    }

    let mut steps = Vec::new();
    for (rel_path, (_language, expected)) in &expected_files {
        let disk_path = app_root.join(rel_path);
        let current = if disk_path.exists() {
            fs::read_to_string(&disk_path)?
        } else {
            String::new()
        };
        if current != *expected {
            steps.push(DependencyFixStep {
                path: disk_path,
                current,
                new: expected.clone(),
            });
        }
    }

    Ok(steps)
}
