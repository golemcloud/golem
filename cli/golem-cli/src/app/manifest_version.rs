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

use crate::log::LogColorize;
use crate::model::app::manifest_metadata_from_yaml_file;
use crate::validation::{ValidatedResult, ValidationBuilder};
use crate::versions;
use itertools::Itertools;
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Clone)]
struct SourceManifestVersion {
    source: PathBuf,
    manifest_version: Option<String>,
}

#[derive(Debug, Clone)]
enum SourceManifestVersionPolicyResult {
    Supported,
    Warn(String),
    Error(String),
}

pub fn validate_manifest_versions(sources: &BTreeSet<PathBuf>) -> ValidatedResult<()> {
    let source_versions = sources
        .iter()
        .map(|source| SourceManifestVersion {
            source: source.to_path_buf(),
            manifest_version: manifest_metadata_from_yaml_file(source).manifest_version,
        })
        .collect::<Vec<_>>();

    let mut validation = ValidationBuilder::new();

    for source_version in &source_versions {
        let source = source_version.source.to_string_lossy().to_string();
        validation.with_context(vec![("source", source)], |validation| {
            match source_manifest_version_policy(source_version) {
                SourceManifestVersionPolicyResult::Supported => {}
                SourceManifestVersionPolicyResult::Warn(message) => validation.add_warn(message),
                SourceManifestVersionPolicyResult::Error(message) => validation.add_error(message),
            }
        });
    }

    add_manifest_version_group_hints(&mut validation, &source_versions);

    validation.build(())
}

fn source_manifest_version_policy(
    source_manifest_version: &SourceManifestVersion,
) -> SourceManifestVersionPolicyResult {
    match source_manifest_version.manifest_version.as_deref() {
        Some(manifest_version) if manifest_version == versions::sdk::MANIFEST => {
            SourceManifestVersionPolicyResult::Supported
        }
        Some(manifest_version) => SourceManifestVersionPolicyResult::Warn(format!(
            "Unsupported application manifest version: {}. Supported version for this CLI is {}. Migration and compatibility hints are not available yet.",
            manifest_version.log_color_highlight(),
            versions::sdk::MANIFEST.log_color_highlight()
        )),
        None => SourceManifestVersionPolicyResult::Error(format!(
            "Missing required {} field in application manifest.",
            "manifestVersion".log_color_highlight()
        )),
    }
}

fn add_manifest_version_group_hints(
    validation: &mut ValidationBuilder,
    source_versions: &[SourceManifestVersion],
) {
    let distinct_versions = source_versions
        .iter()
        .filter_map(|entry| entry.manifest_version.as_ref())
        .collect::<BTreeSet<_>>();

    if distinct_versions.len() > 1 {
        validation.add_warn(format!(
            "Multiple application manifest versions were detected across sources: {}. Future migrations may require aligning them.",
            distinct_versions
                .iter()
                .map(|version| version.log_color_highlight())
                .join(", ")
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::validate_manifest_versions;
    use crate::versions;
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use test_r::test;

    #[test]
    fn manifest_version_check_accepts_supported_version() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = temp_dir.path().join("golem.yaml");
        std::fs::write(
            &source,
            format!("manifestVersion: {}\n", versions::sdk::MANIFEST),
        )
        .unwrap();

        let result = validate_manifest_versions(&BTreeSet::from([source]));
        let (_value, warns, errors) = result.into_product();

        assert!(warns.is_empty(), "unexpected warnings: {warns:?}");
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn manifest_version_check_errors_when_missing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = temp_dir.path().join("golem.yaml");
        std::fs::write(&source, "app: test-app\n").unwrap();

        let result = validate_manifest_versions(&BTreeSet::from([source]));
        let (_value, _warns, errors) = result.into_product();

        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Missing required"));
    }

    #[test]
    fn manifest_version_check_warns_for_mixed_versions() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source_a = temp_dir.path().join("a.yaml");
        let source_b = temp_dir.path().join("b.yaml");

        std::fs::write(
            &source_a,
            format!("manifestVersion: {}\n", versions::sdk::MANIFEST),
        )
        .unwrap();
        std::fs::write(&source_b, "manifestVersion: 9.9.9\n").unwrap();

        let result = validate_manifest_versions(&BTreeSet::from([
            PathBuf::from(source_a),
            PathBuf::from(source_b),
        ]));
        let (_value, warns, errors) = result.into_product();

        assert_eq!(errors.len(), 0);
        assert_eq!(warns.len(), 2);
        assert!(warns
            .iter()
            .any(|warn| warn.contains("Unsupported application manifest version")));
        assert!(warns
            .iter()
            .any(|warn| warn.contains("Multiple application manifest versions were detected")));
    }
}
