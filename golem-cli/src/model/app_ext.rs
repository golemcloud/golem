// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::model::app_ext_raw;
use golem_common::model::{
    ComponentFilePathWithPermissions, ComponentFilePermissions, ComponentType,
};
use golem_wasm_rpc_stubgen::model::app::ComponentPropertiesExtensions;
use golem_wasm_rpc_stubgen::validation::ValidationBuilder;
use serde_json::Value;
use std::path::{Path, PathBuf};
use url::Url;

#[derive(Clone, Debug)]
pub struct GolemComponentExtensions {
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
}

impl Default for GolemComponentExtensions {
    fn default() -> Self {
        Self {
            component_type: ComponentType::Durable,
            files: vec![],
        }
    }
}

impl ComponentPropertiesExtensions for GolemComponentExtensions {
    type Raw = app_ext_raw::GolemComponentPropertiesExt;

    fn raw_from_serde_json(extensions: Value) -> serde_json::Result<Self::Raw> {
        serde_json::from_value(extensions)
    }

    fn convert_and_validate(
        source: &Path,
        validation: &mut ValidationBuilder,
        raw: Self::Raw,
    ) -> Option<Self> {
        convert_component_files(source, validation, raw.files).map(|files| {
            GolemComponentExtensions {
                component_type: raw
                    .component_type
                    .unwrap_or(app_ext_raw::ComponentType::Durable)
                    .into(),
                files,
            }
        })
    }

    fn merge_wit_overrides(
        mut self,
        source: &Path,
        validation: &mut ValidationBuilder,
        overrides: Self::Raw,
    ) -> serde_json::Result<Option<(Self, bool)>> {
        let mut any_errors = false;
        let mut any_overrides = false;

        if let Some(component_type) = overrides.component_type {
            self.component_type = component_type.into();
            any_overrides = true;
        }

        if !overrides.files.is_empty() {
            any_overrides = true;
            match convert_component_files(source, validation, overrides.files) {
                Some(files) => {
                    self.files.extend(files);
                }
                None => {
                    any_errors = true;
                }
            }
        }

        if any_errors {
            Ok(None)
        } else {
            Ok(Some((self, any_overrides)))
        }
    }
}

fn convert_component_file(
    source: &Path,
    validation: &mut ValidationBuilder,
    file: app_ext_raw::InitialComponentFile,
) -> Option<InitialComponentFile> {
    let source_path = DownloadableFile::make(&file.source_path, source)
        .map_err(|err| {
            validation.push_context("source path", file.source_path.to_string());
            validation.add_error(err);
            validation.pop_context();
        })
        .ok()?;

    Some(InitialComponentFile {
        source_path,
        target: ComponentFilePathWithPermissions {
            path: file.target_path,
            permissions: file
                .permissions
                .unwrap_or(ComponentFilePermissions::ReadOnly),
        },
    })
}

fn convert_component_files(
    source: &Path,
    validation: &mut ValidationBuilder,
    files: Vec<app_ext_raw::InitialComponentFile>,
) -> Option<Vec<InitialComponentFile>> {
    let source_count = files.len();

    let files = files
        .into_iter()
        .filter_map(|file| convert_component_file(source, validation, file))
        .collect::<Vec<_>>();

    (files.len() == source_count).then_some(files)
}

/// http, https, file, or protocol relative
#[derive(Clone, Debug)]
pub struct DownloadableFile(Url);

impl DownloadableFile {
    pub fn make(url_string: &str, relative_to: &Path) -> Result<Self, String> {
        // Try to parse the URL as an absolute URL
        let url = Url::parse(url_string).or_else(|_| {
            // If that fails, try to parse it as a relative path
            let canonical_relative_to = relative_to
                .parent()
                .expect("Failed to get parent")
                .canonicalize()
                .map_err(|_| {
                    format!(
                        "Failed to canonicalize relative path: {}",
                        relative_to.display()
                    )
                })?;

            let source_path = canonical_relative_to.join(PathBuf::from(url_string));
            Url::from_file_path(source_path)
                .map_err(|_| "Failed to convert path to URL".to_string())
        })?;

        let source_path_scheme = url.scheme();
        let supported_schemes = ["http", "https", "file", ""];
        if !supported_schemes.contains(&source_path_scheme) {
            return Err(format!(
                "Unsupported source path scheme: {source_path_scheme}"
            ));
        }
        Ok(DownloadableFile(url))
    }

    pub fn as_url(&self) -> &Url {
        &self.0
    }

    pub fn into_url(self) -> Url {
        self.0
    }
}

#[derive(Clone, Debug)]
pub struct InitialComponentFile {
    pub source_path: DownloadableFile,
    pub target: ComponentFilePathWithPermissions,
}

impl From<app_ext_raw::ComponentType> for ComponentType {
    fn from(raw: app_ext_raw::ComponentType) -> Self {
        match raw {
            app_ext_raw::ComponentType::Ephemeral => Self::Ephemeral,
            app_ext_raw::ComponentType::Durable => Self::Durable,
        }
    }
}
