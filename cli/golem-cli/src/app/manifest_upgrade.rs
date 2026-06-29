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

use crate::app::edit::golem_yaml;
use crate::fs;
use crate::model::app::manifest_metadata_from_yaml_file;
use crate::versions;
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct ManifestUpgradeStep {
    pub path: PathBuf,
    pub current: String,
    pub new: String,
}

pub fn plan_manifest_upgrade_steps(
    sources: &BTreeSet<PathBuf>,
) -> anyhow::Result<Vec<ManifestUpgradeStep>> {
    sources
        .iter()
        .map(|path| {
            let current = fs::read_to_string(path)?;
            let mut new = current.clone();

            let metadata = manifest_metadata_from_yaml_file(path);
            if metadata.manifest_version.as_deref() != Some("1.5.0") {
                return Ok((path, current, new));
            }

            new = golem_yaml::set_scalar(&new, &["manifestVersion"], versions::sdk::MANIFEST)?;
            new = golem_yaml::update_existing_schema_references(
                &new,
                crate::manifest_schema_version!(),
            );

            Ok((path, current, new))
        })
        .filter_map(|result: anyhow::Result<_>| match result {
            Ok((path, current, new)) if current != new => Some(Ok(ManifestUpgradeStep {
                path: path.clone(),
                current,
                new,
            })),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::plan_manifest_upgrade_steps;
    use std::collections::BTreeSet;
    use test_r::test;

    #[test]
    fn plan_manifest_upgrade_steps_updates_version_and_existing_schema_refs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = temp_dir.path().join("golem.yaml");
        std::fs::write(
            &source,
            r#"# $schema: https://schema.golem.cloud/app/golem/1.5.0/golem.schema.json
# yaml-language-server: $schema=https://schema.golem.cloud/app/golem/1.5.0/golem.schema.json
manifestVersion: 1.5.0
app: demo
"#,
        )
        .unwrap();

        let steps = plan_manifest_upgrade_steps(&BTreeSet::from([source.clone()])).unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].path, source);
        assert!(steps[0].new.contains("manifestVersion: 1.6.0"));
        assert!(steps[0].new.contains("/1.6.0-dev.3/golem.schema.json"));
        assert!(!steps[0].new.contains("/1.5.0/golem.schema.json"));
    }

    #[test]
    fn plan_manifest_upgrade_steps_leaves_missing_schema_refs_missing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = temp_dir.path().join("golem.yaml");
        std::fs::write(
            &source,
            r#"manifestVersion: 1.5.0
app: demo
"#,
        )
        .unwrap();

        let steps = plan_manifest_upgrade_steps(&BTreeSet::from([source.clone()])).unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].path, source);
        assert!(steps[0].new.contains("manifestVersion: 1.6.0"));
        assert!(!steps[0].new.contains("schema.golem.cloud"));
    }

    #[test]
    fn plan_manifest_upgrade_steps_ignores_schema_ref_drift_without_version_migration() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = temp_dir.path().join("golem.yaml");
        std::fs::write(
            &source,
            r#"# $schema: https://schema.golem.cloud/app/golem/1.5.0/golem.schema.json
manifestVersion: 1.6.0
app: demo
"#,
        )
        .unwrap();

        let steps = plan_manifest_upgrade_steps(&BTreeSet::from([source])).unwrap();

        assert!(steps.is_empty());
    }
}
