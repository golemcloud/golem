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

use crate::fs;
use crate::sdk_versions;
use anyhow::{anyhow, bail, Context};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;
use toml_edit::{value, Array, Item, Table};

const GOLEM_PATH: &str = "GOLEM_PATH";
const GOLEM_RUST_PATH: &str = "GOLEM_RUST_PATH";
const GOLEM_RUST_VERSION: &str = "GOLEM_RUST_VERSION";
const GOLEM_TS_PACKAGES_PATH: &str = "GOLEM_TS_PACKAGES_PATH";
const GOLEM_TS_VERSION: &str = "GOLEM_TS_VERSION";

const SDK_OVERRIDE_KEYS: [&str; 5] = [
    GOLEM_PATH,
    GOLEM_RUST_PATH,
    GOLEM_RUST_VERSION,
    GOLEM_TS_PACKAGES_PATH,
    GOLEM_TS_VERSION,
];

pub const SDK_OVERRIDES_FILE_NAME: &str = ".golem-sdk-overrides";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SdkOverridesTestProfile {
    LocalWorkspace,
    PublishedArtifacts,
}

/// This should be `PublishedArtifacts` in RC and release phase, otherwise `LocalWorkspace`.
pub const SDK_OVERRIDES_DEFAULT_TEST_PROFILE: SdkOverridesTestProfile =
    SdkOverridesTestProfile::LocalWorkspace;

static SDK_OVERRIDES: LazyLock<anyhow::Result<SdkOverrides>> = LazyLock::new(SdkOverrides::load);

#[derive(Debug, Clone, Default)]
pub struct SdkOverrides {
    golem_path: Option<String>,
    pub golem_rust_path: Option<String>,
    pub golem_rust_version: Option<String>,
    pub ts_packages_path: Option<String>,
    pub ts_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RustDependency {
    Path(PathBuf),
    Version(String),
}

pub fn sdk_overrides() -> anyhow::Result<&'static SdkOverrides> {
    SDK_OVERRIDES.as_ref().map_err(|err| anyhow!("{err:#}"))
}

impl SdkOverrides {
    fn local_workspace_test_values(workspace_dir: &Path) -> HashMap<String, String> {
        Self {
            golem_path: None,
            golem_rust_path: Some(join_path(
                &workspace_dir.to_string_lossy(),
                "sdks/rust/golem-rust",
            )),
            golem_rust_version: None,
            ts_packages_path: Some(join_path(
                &workspace_dir.to_string_lossy(),
                "sdks/ts/packages",
            )),
            ts_version: None,
        }
        .to_env_vars()
    }

    pub fn ts_package_dep(&self, package_name: &str) -> String {
        match &self.ts_packages_path {
            Some(ts_packages_path) => format!("{}/{}", ts_packages_path, package_name),
            None => self
                .ts_version
                .as_deref()
                .unwrap_or(sdk_versions::sdk::TS)
                .to_string(),
        }
    }

    pub fn golem_rust_dep(&self) -> String {
        self.golem_rust_dependency().as_dep_string()
    }

    pub fn golem_rust_dependency(&self) -> RustDependency {
        match &self.golem_rust_path {
            Some(rust_path) => RustDependency::Path(PathBuf::from(rust_path)),
            None => RustDependency::Version(
                self.golem_rust_version
                    .clone()
                    .unwrap_or_else(|| sdk_versions::sdk::RUST.to_string()),
            ),
        }
    }

    pub fn golem_client_dep(&self) -> anyhow::Result<String> {
        if let Some(repo_root) = self.golem_repo_root()? {
            let repo_root = fs::path_to_str(&repo_root)?;
            return Ok(format!(r#"path = "{}/golem-client""#, repo_root));
        }

        todo!("No published version yet")
    }

    pub fn golem_repo_root(&self) -> anyhow::Result<Option<PathBuf>> {
        if let Some(golem_path) = &self.golem_path {
            return Ok(Some(PathBuf::from(golem_path)));
        }

        self.golem_rust_path
            .as_ref()
            .map(|path| Self::golem_repo_root_from_rust_sdk_path(Path::new(path)))
            .transpose()
    }

    fn golem_repo_root_from_rust_sdk_path(path: &Path) -> anyhow::Result<PathBuf> {
        let normalized = normalize_path(path);
        let expected_suffix = Path::new("sdks/rust/golem-rust");

        let normalized_components: Vec<_> = normalized.components().collect();
        let suffix_components: Vec<_> = expected_suffix.components().collect();

        if normalized_components.len() < suffix_components.len()
            || normalized_components[normalized_components.len() - suffix_components.len()..]
                != suffix_components
        {
            bail!(
                "Invalid Golem Rust path: {} (expected suffix {})",
                normalized.display(),
                expected_suffix.display()
            );
        }

        let mut repo_root = PathBuf::new();
        for component in
            &normalized_components[..normalized_components.len() - suffix_components.len()]
        {
            repo_root.push(component.as_os_str());
        }

        if repo_root.as_os_str().is_empty() {
            bail!("Invalid Golem Rust path: missing repo root");
        }

        Ok(repo_root)
    }

    fn load() -> anyhow::Result<Self> {
        let current_dir = std::env::current_dir().context("Failed to resolve current directory")?;
        let file_values = Self::load_file_values(&current_dir)?;

        let test_values = Self::load_test_values()?;

        let env_values = Self::load_env_values();

        Ok(Self::from_values_with_test(
            file_values,
            test_values,
            env_values,
        ))
    }

    fn load_test_values() -> anyhow::Result<HashMap<String, String>> {
        if !should_apply_test_layer()
            || SDK_OVERRIDES_DEFAULT_TEST_PROFILE == SdkOverridesTestProfile::PublishedArtifacts
            || !running_from_golem_workspace_checkout()
        {
            Ok(HashMap::new())
        } else {
            Ok(Self::local_workspace_test_values(&workspace_root()?))
        }
    }

    #[cfg(test)]
    fn from_values(
        file_values: HashMap<String, String>,
        env_values: HashMap<String, String>,
    ) -> Self {
        Self::from_values_with_test(file_values, HashMap::new(), env_values)
    }

    fn from_values_with_test(
        file_values: HashMap<String, String>,
        test_values: HashMap<String, String>,
        env_values: HashMap<String, String>,
    ) -> Self {
        let mut values = file_values;
        values.extend(test_values);
        values.extend(env_values);

        let golem_path = get_normalized_value_by_key(&values, GOLEM_PATH);

        Self {
            golem_path: golem_path.clone(),
            golem_rust_path: get_normalized_value_by_key(&values, GOLEM_RUST_PATH).or_else(|| {
                golem_path
                    .as_deref()
                    .map(|path| join_path(path, "sdks/rust/golem-rust"))
            }),
            golem_rust_version: get_normalized_value_by_key(&values, GOLEM_RUST_VERSION),
            ts_packages_path: get_normalized_value_by_key(&values, GOLEM_TS_PACKAGES_PATH).or_else(
                || {
                    golem_path
                        .as_deref()
                        .map(|path| join_path(path, "sdks/ts/packages"))
                },
            ),
            ts_version: get_normalized_value_by_key(&values, GOLEM_TS_VERSION),
        }
    }

    pub fn to_env_vars(&self) -> HashMap<String, String> {
        let mut values = HashMap::new();
        if let Some(value) = &self.golem_rust_path {
            values.insert(GOLEM_RUST_PATH.to_string(), value.clone());
        }
        if let Some(value) = &self.golem_rust_version {
            values.insert(GOLEM_RUST_VERSION.to_string(), value.clone());
        }
        if let Some(value) = &self.ts_packages_path {
            values.insert(GOLEM_TS_PACKAGES_PATH.to_string(), value.clone());
        }
        if let Some(value) = &self.ts_version {
            values.insert(GOLEM_TS_VERSION.to_string(), value.clone());
        }
        values
    }

    fn load_env_values() -> HashMap<String, String> {
        SDK_OVERRIDE_KEYS
            .iter()
            .filter_map(|key| {
                std::env::var(key)
                    .ok()
                    .map(|value| ((*key).to_string(), value))
            })
            .collect()
    }

    fn load_file_values(current_dir: &Path) -> anyhow::Result<HashMap<String, String>> {
        let Some(overrides_file) = find_sdk_overrides_file(current_dir) else {
            return Ok(HashMap::new());
        };

        let mut values = parse_dotenv_with_relative_paths(&overrides_file)?;
        if values.is_empty() {
            let parent = fs::parent_or_err(&overrides_file)?;
            values.insert(GOLEM_PATH.to_string(), fs::path_to_str(parent)?.to_string());
        }

        Ok(values)
    }
}

impl RustDependency {
    pub fn as_dep_string(&self) -> String {
        match self {
            RustDependency::Path(path) => format!(r#"path = "{}""#, path.to_string_lossy()),
            RustDependency::Version(version) => format!(r#"version = "{}""#, version),
        }
    }

    pub fn as_toml_item(&self, features: &[&str]) -> Item {
        let mut entry = Item::Table(Table::default());
        match self {
            RustDependency::Path(path) => {
                entry["path"] = value(path.to_string_lossy().to_string());
            }
            RustDependency::Version(version) => {
                entry["version"] = value(version.clone());
            }
        }

        if !features.is_empty() {
            let mut feature_items = Array::default();
            for feature in features {
                feature_items.push(*feature);
            }
            entry["default-features"] = value(false);
            entry["features"] = value(feature_items);
        }

        entry
    }
}

pub fn workspace_root() -> anyhow::Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fs::canonicalize_path(&manifest_dir.join("../.."))
}

fn running_from_golem_workspace_checkout() -> bool {
    let Some(workspace_root) = workspace_root().ok() else {
        return false;
    };

    if !has_local_workspace_sdks(&workspace_root) {
        return false;
    }

    let current_dir = std::env::current_dir().ok();
    let current_exe = std::env::current_exe().ok();

    current_dir
        .as_deref()
        .is_some_and(|path| path.starts_with(&workspace_root))
        || current_exe
            .as_deref()
            .is_some_and(|path| path.starts_with(&workspace_root))
}

fn should_apply_test_layer() -> bool {
    cfg!(debug_assertions)
}

fn has_local_workspace_sdks(workspace_root: &Path) -> bool {
    workspace_root.join("sdks/rust/golem-rust").is_dir()
        && workspace_root.join("sdks/ts/packages").is_dir()
}

fn find_sdk_overrides_file(start_dir: &Path) -> Option<PathBuf> {
    start_dir
        .ancestors()
        .map(|dir| dir.join(SDK_OVERRIDES_FILE_NAME))
        .find(|candidate| candidate.exists())
}

fn parse_dotenv_with_relative_paths(path: &Path) -> anyhow::Result<HashMap<String, String>> {
    let file_dir = fs::parent_or_err(path)?;

    let mut values = HashMap::new();
    for entry in dotenvy::from_path_iter(path)
        .with_context(|| format!("Failed to parse {}", path.display()))?
    {
        let (key, value) = entry.with_context(|| format!("Failed to parse {}", path.display()))?;
        let value = if is_path_override_key(&key) {
            resolve_relative_path(file_dir, &value)
        } else {
            value
        };
        values.insert(key, value);
    }

    Ok(values)
}

fn is_path_override_key(key: &str) -> bool {
    matches!(key, GOLEM_PATH | GOLEM_RUST_PATH | GOLEM_TS_PACKAGES_PATH)
}

fn resolve_relative_path(base_dir: &Path, value: &str) -> String {
    let path = Path::new(value);
    if path.is_absolute() {
        normalize_path(path).to_string_lossy().to_string()
    } else {
        normalize_path(&base_dir.join(path))
            .to_string_lossy()
            .to_string()
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn get_normalized_value_by_key(values: &HashMap<String, String>, key: &str) -> Option<String> {
    values.get(key).and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then_some(trimmed.to_string())
    })
}

fn join_path(base: &str, suffix: &str) -> String {
    Path::new(base).join(suffix).to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use test_r::test;

    fn map(items: &[(&str, &str)]) -> HashMap<String, String> {
        items
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn env_values_override_file_values() {
        let file_values = map(&[(GOLEM_RUST_VERSION, "1.2.3")]);
        let env_values = map(&[(GOLEM_RUST_VERSION, "9.9.9")]);

        let overrides = SdkOverrides::from_values(file_values, env_values);
        assert_eq!(overrides.golem_rust_version.as_deref(), Some("9.9.9"));
    }

    #[test]
    fn golem_path_sets_default_sdk_paths() {
        let overrides = SdkOverrides::from_values(map(&[(GOLEM_PATH, "/repo")]), HashMap::new());

        assert_eq!(
            overrides.golem_rust_path.as_deref(),
            Some("/repo/sdks/rust/golem-rust")
        );
        assert_eq!(
            overrides.ts_packages_path.as_deref(),
            Some("/repo/sdks/ts/packages")
        );
    }

    #[test]
    fn explicit_sdk_paths_win_over_golem_path() {
        let overrides = SdkOverrides::from_values(
            map(&[
                (GOLEM_PATH, "/repo"),
                (GOLEM_RUST_PATH, "/custom/rust"),
                (GOLEM_TS_PACKAGES_PATH, "/custom/ts"),
            ]),
            HashMap::new(),
        );

        assert_eq!(overrides.golem_rust_path.as_deref(), Some("/custom/rust"));
        assert_eq!(overrides.ts_packages_path.as_deref(), Some("/custom/ts"));
    }

    #[test]
    fn env_values_override_test_profile_values() {
        let test_values = SdkOverrides {
            golem_path: None,
            golem_rust_path: Some("/repo/sdks/rust/golem-rust".to_string()),
            golem_rust_version: None,
            ts_packages_path: Some("/repo/sdks/ts/packages".to_string()),
            ts_version: None,
        }
        .to_env_vars();

        let env_values = map(&[(GOLEM_RUST_PATH, "/custom/rust")]);

        let overrides =
            SdkOverrides::from_values_with_test(HashMap::new(), test_values, env_values);

        assert_eq!(overrides.golem_rust_path.as_deref(), Some("/custom/rust"));
    }

    #[test]
    fn parses_relative_paths_from_overrides_file() {
        let temp = tempdir().expect("failed to create temp dir");
        let root = temp.path().join("repo");
        let child = root.join("nested");
        std::fs::create_dir_all(&child).expect("failed to create nested dir");

        let file = child.join(SDK_OVERRIDES_FILE_NAME);
        std::fs::write(
            &file,
            "GOLEM_PATH=..\nGOLEM_RUST_PATH=./rust-override\nGOLEM_TS_PACKAGES_PATH=../ts-override\n",
        )
            .expect("failed to write overrides file");

        let values =
            parse_dotenv_with_relative_paths(&file).expect("failed to parse overrides file");
        assert_eq!(
            values.get(GOLEM_PATH),
            Some(&root.to_string_lossy().to_string())
        );
        assert_eq!(
            values.get(GOLEM_RUST_PATH),
            Some(&child.join("rust-override").to_string_lossy().to_string())
        );
        assert_eq!(
            values.get(GOLEM_TS_PACKAGES_PATH),
            Some(&root.join("ts-override").to_string_lossy().to_string())
        );
    }

    #[test]
    fn finds_nearest_overrides_file() {
        let temp = tempdir().expect("failed to create temp dir");
        let root = temp.path().join("root");
        let child = root.join("a/b/c");
        std::fs::create_dir_all(&child).expect("failed to create nested dirs");

        std::fs::write(root.join(SDK_OVERRIDES_FILE_NAME), "GOLEM_PATH=/root\n")
            .expect("failed to write root overrides file");
        std::fs::write(
            root.join("a").join(SDK_OVERRIDES_FILE_NAME),
            "GOLEM_PATH=/a\n",
        )
        .expect("failed to write child overrides file");

        let found = find_sdk_overrides_file(&child).expect("no overrides file found");
        assert_eq!(found, root.join("a").join(SDK_OVERRIDES_FILE_NAME));
    }

    #[test]
    fn golem_repo_root_is_derived_structurally_from_rust_path() {
        let rust_path = Path::new("/tmp/workspace/sdks/rust/golem-rust");
        let root = SdkOverrides::golem_repo_root_from_rust_sdk_path(rust_path)
            .expect("expected to derive repo root");

        assert_eq!(root, PathBuf::from("/tmp/workspace"));
    }

    #[test]
    fn golem_repo_root_derivation_fails_for_non_matching_path() {
        let rust_path = Path::new("/tmp/workspace/sdks/rust/not-golem-rust");
        let err = SdkOverrides::golem_repo_root_from_rust_sdk_path(rust_path)
            .expect_err("expected invalid path error");

        assert!(format!("{err:#}").contains("Invalid Golem Rust path"));
    }

    #[test]
    fn rust_dependency_can_be_rendered_for_toml_edit() {
        let dep = RustDependency::Path(PathBuf::from("/tmp/repo/sdks/rust/golem-rust"));
        let item = dep.as_toml_item(&["client"]);

        assert_eq!(
            item["path"].as_str(),
            Some("/tmp/repo/sdks/rust/golem-rust")
        );
        assert_eq!(item["default-features"].as_bool(), Some(false));
        assert_eq!(item["features"].as_array().map(|a| a.len()), Some(1));
    }

    #[test]
    fn empty_overrides_file_defaults_golem_path_to_file_dir() {
        let temp = tempdir().expect("failed to create temp dir");
        let root = temp.path().join("repo");
        let child = root.join("nested");
        std::fs::create_dir_all(&child).expect("failed to create nested dirs");

        std::fs::write(
            root.join(SDK_OVERRIDES_FILE_NAME),
            "\n# only comments and whitespace\n   \n",
        )
        .expect("failed to write overrides file");

        let file_values =
            SdkOverrides::load_file_values(&child).expect("failed to load file values");

        assert_eq!(
            file_values.get(GOLEM_PATH),
            Some(&root.to_string_lossy().to_string())
        );

        let overrides = SdkOverrides::from_values(file_values, HashMap::new());
        let expected_rust_path = root
            .join("sdks/rust/golem-rust")
            .to_string_lossy()
            .to_string();
        assert_eq!(
            overrides.golem_rust_path.as_deref(),
            Some(expected_rust_path.as_str())
        );
    }

    #[test]
    fn non_empty_overrides_file_does_not_infer_golem_path() {
        let temp = tempdir().expect("failed to create temp dir");
        let root = temp.path().join("repo");
        let child = root.join("nested");
        std::fs::create_dir_all(&child).expect("failed to create nested dirs");

        std::fs::write(
            root.join(SDK_OVERRIDES_FILE_NAME),
            "GOLEM_RUST_VERSION=1.2.3\n",
        )
        .expect("failed to write overrides file");

        let file_values =
            SdkOverrides::load_file_values(&child).expect("failed to load file values");
        assert!(!file_values.contains_key(GOLEM_PATH));
    }
}
