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
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::LazyLock;

static SUPPORTED_MANIFEST_VERSION: LazyLock<(u64, u64, u64)> = LazyLock::new(|| {
    parse_strict_manifest_version(versions::sdk::MANIFEST).unwrap_or_else(|| {
        panic!(
            "Invalid supported manifest version literal '{}': expected strict x.y.z",
            versions::sdk::MANIFEST
        )
    })
});

#[derive(Debug, Clone)]
struct SourceManifestVersion {
    source: PathBuf,
    manifest_version: Option<String>,
}

#[derive(Debug, Clone)]
enum SourceManifestVersionPolicyResult {
    Supported,
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManifestVersionStatus {
    Supported,
    TooOld,
    TooNew,
    Invalid,
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

    if let Some(conflict_error) = mixed_manifest_versions_error(&source_versions) {
        validation.add_error(conflict_error);
        return validation.build(());
    }

    for source_version in &source_versions {
        let source = source_version.source.to_string_lossy().to_string();
        validation.with_context(vec![("source", source)], |validation| {
            match source_manifest_version_policy(source_version) {
                SourceManifestVersionPolicyResult::Supported => {}
                SourceManifestVersionPolicyResult::Error(message) => validation.add_error(message),
            }
        });
    }

    validation.build(())
}

fn mixed_manifest_versions_error(source_versions: &[SourceManifestVersion]) -> Option<String> {
    let mut by_version: BTreeMap<&str, Vec<String>> = BTreeMap::new();

    for source_version in source_versions {
        if let Some(manifest_version) = source_version.manifest_version.as_deref() {
            by_version
                .entry(manifest_version)
                .or_default()
                .push(source_version.source.to_string_lossy().to_string());
        }
    }

    if by_version.len() <= 1 {
        return None;
    }

    let grouped = by_version
        .into_iter()
        .map(|(version, sources)| {
            format!(
                "{} [{}]",
                version.log_color_highlight(),
                sources
                    .into_iter()
                    .map(|source| source.log_color_highlight().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join("; ");

    Some(format!(
        "Conflicting {} values across application manifests: {}.",
        "manifestVersion".log_color_highlight(),
        grouped
    ))
}

fn source_manifest_version_policy(
    source_manifest_version: &SourceManifestVersion,
) -> SourceManifestVersionPolicyResult {
    match source_manifest_version.manifest_version.as_deref() {
        None => SourceManifestVersionPolicyResult::Error(format!(
            "Missing required {} field in application manifest. Recreate the application with {} in a new directory, then move existing agent code to the new layout.",
            "manifestVersion".log_color_highlight()
            ,"golem new".log_color_highlight()
        )),
        Some(manifest_version) => match manifest_version_status(manifest_version) {
            ManifestVersionStatus::Supported => SourceManifestVersionPolicyResult::Supported,
            ManifestVersionStatus::TooOld | ManifestVersionStatus::Invalid => {
                SourceManifestVersionPolicyResult::Error(format!(
                    "Unknown application manifest version: {}. This CLI supports {}.",
                    manifest_version.log_color_highlight(),
                    versions::sdk::MANIFEST.log_color_highlight()
                ))
            }
            ManifestVersionStatus::TooNew => SourceManifestVersionPolicyResult::Error(format!(
                "Application manifest version {} is newer than supported by this CLI ({}). Please update your CLI.",
                manifest_version.log_color_highlight(),
                versions::sdk::MANIFEST.log_color_highlight()
            )),
        },
    }
}

fn manifest_version_status(manifest_version: &str) -> ManifestVersionStatus {
    let Some(actual) = parse_strict_manifest_version(manifest_version) else {
        return ManifestVersionStatus::Invalid;
    };
    let supported = *SUPPORTED_MANIFEST_VERSION;

    if actual == supported {
        ManifestVersionStatus::Supported
    } else if actual < supported {
        ManifestVersionStatus::TooOld
    } else {
        ManifestVersionStatus::TooNew
    }
}

fn parse_strict_manifest_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;

    if parts.next().is_some() {
        return None;
    }

    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::validate_manifest_versions;
    use crate::versions;
    use std::collections::BTreeSet;
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
        let (_value, warns, errors) = result.into_product();

        assert!(warns.is_empty(), "unexpected warnings: {warns:?}");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Missing required"));
        assert!(errors[0].contains("golem new"));
    }

    #[test]
    fn manifest_version_check_errors_for_older_version() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = temp_dir.path().join("golem.yaml");

        std::fs::write(&source, "manifestVersion: 1.4.9\n").unwrap();

        let result = validate_manifest_versions(&BTreeSet::from([source]));
        let (_value, warns, errors) = result.into_product();

        assert!(warns.is_empty(), "unexpected warnings: {warns:?}");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Unknown application manifest version"));
    }

    #[test]
    fn manifest_version_check_errors_for_newer_version() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = temp_dir.path().join("golem.yaml");

        std::fs::write(&source, "manifestVersion: 1.5.1\n").unwrap();

        let result = validate_manifest_versions(&BTreeSet::from([source]));
        let (_value, warns, errors) = result.into_product();

        assert!(warns.is_empty(), "unexpected warnings: {warns:?}");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Please update your CLI"));
    }

    #[test]
    fn manifest_version_check_errors_for_invalid_version() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = temp_dir.path().join("golem.yaml");

        std::fs::write(&source, "manifestVersion: not-a-version\n").unwrap();

        let result = validate_manifest_versions(&BTreeSet::from([source]));
        let (_value, warns, errors) = result.into_product();

        assert!(warns.is_empty(), "unexpected warnings: {warns:?}");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Unknown application manifest version"));
    }

    #[test]
    fn manifest_version_check_errors_when_sources_have_mixed_versions() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source_a = temp_dir.path().join("a.yaml");
        let source_b = temp_dir.path().join("b.yaml");

        std::fs::write(
            &source_a,
            format!("manifestVersion: {}\n", versions::sdk::MANIFEST),
        )
        .unwrap();
        std::fs::write(&source_b, "manifestVersion: 1.5.1\n").unwrap();

        let result = validate_manifest_versions(&BTreeSet::from([source_a, source_b]));
        let (_value, warns, errors) = result.into_product();

        assert!(warns.is_empty(), "unexpected warnings: {warns:?}");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Conflicting"));
        assert!(errors[0].contains("manifestVersion"));
        assert!(errors[0].contains(versions::sdk::MANIFEST));
        assert!(errors[0].contains("1.5.1"));
    }
}
