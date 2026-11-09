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
use crate::model::app_raw::{Application, ApplicationMetadata};
use crate::{fs, versions};
use anyhow::{Context, bail, ensure};
use serde_yaml::Value;
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
    let contents = sources
        .iter()
        .map(|path| {
            let current = fs::read_to_string(path)?;
            let metadata = ApplicationMetadata::from_yaml_str(&current).with_context(|| {
                format!("Failed to read manifest version in {}", path.display())
            })?;
            Ok((path, current, metadata.manifest_version))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let upgrades_from_1_5 = contents
        .iter()
        .any(|(_, _, version)| version.as_deref() == Some("1.5.0"));
    if !contents
        .iter()
        .any(|(_, _, version)| matches!(version.as_deref(), Some("1.5.0" | "1.6.0")))
    {
        return Ok(Vec::new());
    }

    let mut steps = Vec::new();
    let mut staged_paths = BTreeSet::new();
    for (path, current, version) in contents {
        ensure!(
            matches!(version.as_deref(), None | Some("1.5.0" | "1.6.0"))
                || version.as_deref() == Some(versions::sdk::MANIFEST),
            "Cannot automatically upgrade {} with unknown manifest version {:?}",
            path.display(),
            version
        );
        let value: Value = serde_yaml::from_str(&current)?;
        if upgrades_from_1_5 && value.get("bridge").is_some() {
            bail!(
                "Cannot automatically upgrade {} because bridge SDK manifest shape changed; move bridge language agents/outputDir under external before retrying the upgrade",
                path.display()
            );
        }
        if version.as_deref() == Some(versions::sdk::MANIFEST) {
            continue;
        }

        let mut new = current.clone();
        for section in ["components", "componentTemplates"] {
            let Some(components) = value[section].as_mapping() else {
                continue;
            };
            for (name, properties) in components {
                let name = name.as_str().context("Component name must be a string")?;
                let mut property_sets = vec![(vec![section, name], properties)];
                if let Some(presets) = properties["presets"].as_mapping() {
                    for (preset, properties) in presets {
                        let preset = preset.as_str().context("Preset name must be a string")?;
                        property_sets.push((vec![section, name, "presets", preset], properties));
                    }
                }
                for (property_path, properties) in property_sets {
                    if properties.get("outputWasm").is_some() {
                        if let Some(path) = properties["outputWasm"].as_str()
                            && Some(path) != properties["componentWasm"].as_str()
                        {
                            staged_paths.insert(path.to_string());
                        }
                        new = golem_yaml::remove_map_entry(&new, &property_path, "outputWasm")
                            .with_context(|| format!("Cannot upgrade {}", path.display()))?;
                    }
                }
            }
        }
        new = if version.is_some() {
            golem_yaml::set_scalar(&new, &["manifestVersion"], versions::sdk::MANIFEST)?
        } else {
            golem_yaml::add_map_entry(&new, &[], "manifestVersion", versions::sdk::MANIFEST)?
        };
        new =
            golem_yaml::update_existing_schema_references(&new, crate::manifest_schema_version!());
        Application::from_yaml_str(&new).with_context(|| {
            format!(
                "Cannot upgrade {} to the current manifest format",
                path.display()
            )
        })?;
        steps.push(ManifestUpgradeStep {
            path: path.clone(),
            current,
            new,
        });
    }

    for path in sources {
        let new = match steps.iter().find(|step| &step.path == path) {
            Some(step) => step.new.clone(),
            None => fs::read_to_string(path)?,
        };
        let value: Value = serde_yaml::from_str(&new)?;
        for staged in &staged_paths {
            ensure!(
                !references_path(&value, staged),
                "Cannot automatically upgrade {}: configuration still references removed outputWasm path '{}'; update these references to the componentWasm artifact for each preset before retrying",
                path.display(),
                staged
            );
        }
    }
    Ok(steps)
}

fn references_path(value: &Value, path: &str) -> bool {
    match value {
        Value::String(value) => value.contains(path),
        Value::Sequence(values) => values.iter().any(|value| references_path(value, path)),
        Value::Mapping(values) => values.values().any(|value| references_path(value, path)),
        Value::Tagged(value) => references_path(&value.value, path),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::plan_manifest_upgrade_steps;
    use crate::model::app_raw::Application;
    use indoc::indoc;
    use serde_yaml::Value;
    use std::collections::BTreeSet;
    use test_r::test;

    #[test]
    fn manifest_upgrade_preserves_artifacts_templates_presets_and_comments() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("golem.yaml");
        let fragment = dir.path().join("templates.yaml");
        for version in ["1.5.0", "1.6.0"] {
            let original = format!(
                "# $schema: https://schema.golem.cloud/app/golem/{version}/golem.schema.json\n# yaml-language-server: $schema=https://schema.golem.cloud/app/golem/{version}/golem.schema.json\nmanifestVersion: {version}\napp: example\nincludes: [templates.yaml]\ncomponents:\n  app:main:\n    templates: shared\n    componentWasm: built.wasm # Keep the actual artifact\n    outputWasm: staged.wasm\n    presets:\n      quick:\n        componentWasm: dynamic.wasm\n        outputWasm: quick-staged.wasm\n      inherited:\n        outputWasm: unused.wasm\n"
            );
            std::fs::write(&root, &original).unwrap();
            std::fs::write(
                &fragment,
                indoc! {"
                    # Included template without a version marker
                    componentTemplates:
                      shared:
                        componentWasm: template.wasm
                        outputWasm: template-staged.wasm
                        presets:
                          optimized:
                            componentWasm: preinitialized.wasm
                            outputWasm: optimized-staged.wasm
                "},
            )
            .unwrap();
            let sources = BTreeSet::from([root.clone(), fragment.clone()]);
            let steps = plan_manifest_upgrade_steps(&sources).unwrap();
            assert_eq!(steps.len(), 2);
            assert_eq!(std::fs::read_to_string(&root).unwrap(), original);
            for step in steps {
                Application::from_yaml_str(&step.new).unwrap();
                assert!(!step.new.contains("outputWasm"));
                let value: Value = serde_yaml::from_str(&step.new).unwrap();
                assert_eq!(value["manifestVersion"], "2.0.0");
                if step.path == root {
                    assert!(step.new.contains("# Keep the actual artifact"));
                    assert_eq!(
                        step.new.matches("/2.0.0-dev.1/golem.schema.json").count(),
                        2
                    );
                    let component = &value["components"]["app:main"];
                    assert_eq!(component["componentWasm"], "built.wasm");
                    assert_eq!(
                        component["presets"]["quick"]["componentWasm"],
                        "dynamic.wasm"
                    );
                    assert_eq!(
                        component["presets"]["inherited"],
                        Value::Mapping(Default::default())
                    );
                } else {
                    assert!(!step.new.contains("schema.golem.cloud"));
                    assert!(
                        step.new
                            .contains("# Included template without a version marker")
                    );
                    let template = &value["componentTemplates"]["shared"];
                    assert_eq!(template["componentWasm"], "template.wasm");
                    assert_eq!(
                        template["presets"]["optimized"]["componentWasm"],
                        "preinitialized.wasm"
                    );
                }
                std::fs::write(step.path, step.new).unwrap();
            }
            assert!(plan_manifest_upgrade_steps(&sources).unwrap().is_empty());
        }
    }

    #[test]
    fn manifest_upgrade_rejects_cross_file_staged_path_references_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("golem.yaml");
        let fragment = dir.path().join("commands.yaml");
        let original = indoc! {"
            manifestVersion: 1.6.0
            app: example
            includes: [commands.yaml]
            components:
              app:main:
                componentWasm: built.wasm
                outputWasm: staged.wasm
        "};
        std::fs::write(&root, original).unwrap();
        for version in ["1.6.0", "2.0.0"] {
            let commands = format!(
                "manifestVersion: {version}\ncustomCommands:\n  copy:\n  - command: cp staged.wasm export.wasm\n"
            );
            std::fs::write(&fragment, &commands).unwrap();
            let error =
                plan_manifest_upgrade_steps(&BTreeSet::from([root.clone(), fragment.clone()]))
                    .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("removed outputWasm path 'staged.wasm'")
            );
            assert!(error.to_string().contains("commands.yaml"));
            assert_eq!(std::fs::read_to_string(&root).unwrap(), original);
            assert_eq!(std::fs::read_to_string(&fragment).unwrap(), commands);
        }
    }

    #[test]
    fn manifest_upgrade_does_not_treat_actual_artifact_as_removed_staging() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("golem.yaml");
        std::fs::write(
            &root,
            indoc! {"
            manifestVersion: 1.6.0
            components:
              app:main:
                componentWasm: built.wasm
                outputWasm: built.wasm
                customCommands:
                  copy:
                  - command: cp built.wasm export.wasm
        "},
        )
        .unwrap();
        let steps = plan_manifest_upgrade_steps(&BTreeSet::from([root])).unwrap();
        assert!(steps[0].new.contains("cp built.wasm export.wasm"));
        assert!(!steps[0].new.contains("outputWasm"));
    }

    #[test]
    fn manifest_upgrade_rejects_staging_that_is_only_another_presets_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("golem.yaml");
        std::fs::write(
            &root,
            indoc! {"
            manifestVersion: 1.6.0
            components:
              app:main:
                presets:
                  optimized:
                    componentWasm: optimized.wasm
                    outputWasm: shared.wasm
                  quick:
                    componentWasm: shared.wasm
                    outputWasm: shared.wasm
                customCommands:
                  copy:
                  - command: cp shared.wasm export.wasm
        "},
        )
        .unwrap();
        let error = plan_manifest_upgrade_steps(&BTreeSet::from([root])).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("removed outputWasm path 'shared.wasm'")
        );
    }

    #[test]
    fn manifest_upgrade_preserves_bridge_guard_for_versionless_fragments() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("golem.yaml");
        let fragment = dir.path().join("bridge.yaml");
        std::fs::write(&root, "manifestVersion: 1.5.0\napp: example\n").unwrap();
        std::fs::write(
            &fragment,
            "bridge:\n  rust:\n    agents: CounterAgent\n    outputDir: bridge/rust\n",
        )
        .unwrap();
        let error = plan_manifest_upgrade_steps(&BTreeSet::from([root, fragment])).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("bridge SDK manifest shape changed")
        );
    }

    #[test]
    fn manifest_upgrade_does_not_rewrite_unknown_or_current_versions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("golem.yaml");
        let fragment = dir.path().join("unknown.yaml");
        for version in ["1.4.0", "1.6.1", "3.0.0", "invalid", "2.0.0"] {
            let original = format!(
                "# $schema: https://schema.golem.cloud/app/golem/1.6.0/golem.schema.json\nmanifestVersion: {version}\napp: example\n"
            );
            std::fs::write(&fragment, &original).unwrap();
            assert!(
                plan_manifest_upgrade_steps(&BTreeSet::from([fragment.clone()]))
                    .unwrap()
                    .is_empty()
            );
            if version != "2.0.0" {
                std::fs::write(&root, "manifestVersion: 1.6.0\napp: example\n").unwrap();
                assert!(
                    plan_manifest_upgrade_steps(&BTreeSet::from([root.clone(), fragment.clone()]))
                        .is_err()
                );
            }
            assert_eq!(std::fs::read_to_string(&fragment).unwrap(), original);
        }
    }
}
