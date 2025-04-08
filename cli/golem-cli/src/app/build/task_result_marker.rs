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

use crate::fs;
use crate::model::app::{AppComponentName, DependentComponent};
use crate::model::app_raw;
use anyhow::{anyhow, Context};
use itertools::Itertools;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use wit_parser::PackageName;

pub trait TaskResultMarkerHashInput {
    fn task_kind() -> &'static str;

    fn hash_input(&self) -> anyhow::Result<Vec<u8>>;
}

#[derive(Serialize)]
pub struct ResolvedExternalCommandMarkerHash<'a> {
    pub build_dir: &'a Path,
    pub command: &'a app_raw::ExternalCommand,
}

impl TaskResultMarkerHashInput for ResolvedExternalCommandMarkerHash<'_> {
    fn task_kind() -> &'static str {
        "ResolvedExternalCommandMarkerHash"
    }

    fn hash_input(&self) -> anyhow::Result<Vec<u8>> {
        Ok(serde_yaml::to_string(self)?.into_bytes())
    }
}

pub struct ComponentGeneratorMarkerHash<'a> {
    pub component_name: &'a AppComponentName,
    pub generator_kind: &'a str,
}

impl TaskResultMarkerHashInput for ComponentGeneratorMarkerHash<'_> {
    fn task_kind() -> &'static str {
        "ComponentGeneratorMarkerHash"
    }

    fn hash_input(&self) -> anyhow::Result<Vec<u8>> {
        Ok(format!("{}-{}", self.component_name, self.generator_kind).into_bytes())
    }
}

pub struct LinkRpcMarkerHash<'a> {
    pub component_name: &'a AppComponentName,
    pub dependencies: &'a BTreeSet<&'a DependentComponent>,
}

impl TaskResultMarkerHashInput for LinkRpcMarkerHash<'_> {
    fn task_kind() -> &'static str {
        "RpcLinkMarkerHash"
    }

    fn hash_input(&self) -> anyhow::Result<Vec<u8>> {
        Ok(format!(
            "{}#{}",
            self.component_name,
            self.dependencies
                .iter()
                .map(|s| format!("{}#{}", s.name.as_str(), s.dep_type.as_str()))
                .join(",")
        )
        .into_bytes())
    }
}

pub struct AddMetadataMarkerHash<'a> {
    pub component_name: &'a AppComponentName,
    pub root_package_name: PackageName,
}

impl TaskResultMarkerHashInput for AddMetadataMarkerHash<'_> {
    fn task_kind() -> &'static str {
        "AddMetadataMarkerHash"
    }

    fn hash_input(&self) -> anyhow::Result<Vec<u8>> {
        Ok(format!("{}#{}", self.component_name, self.root_package_name).into_bytes())
    }
}

pub struct TaskResultMarker {
    success_marker_file_path: PathBuf,
    failure_marker_file_path: PathBuf,
    success_before: bool,
    failure_before: bool,
}

static TASK_RESULT_MARKER_SUCCESS_SUFFIX: &str = "-success";
static TASK_RESULT_MARKER_FAILURE_SUFFIX: &str = "-failure";

impl TaskResultMarker {
    pub fn new<T: TaskResultMarkerHashInput>(dir: &Path, task: T) -> anyhow::Result<Self> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(T::task_kind().as_bytes());
        hasher.update(&task.hash_input()?);
        let hex_hash = hasher.finalize().to_hex().to_string();

        let success_marker_file_path = dir.join(format!(
            "{}{}",
            &hex_hash, TASK_RESULT_MARKER_SUCCESS_SUFFIX
        ));
        let failure_marker_file_path = dir.join(format!(
            "{}{}",
            &hex_hash, TASK_RESULT_MARKER_FAILURE_SUFFIX
        ));

        let success_marker_exists = success_marker_file_path.exists();
        let failure_marker_exists = failure_marker_file_path.exists();

        let (success_before, failure_before) = match (success_marker_exists, failure_marker_exists)
        {
            (true, false) => (true, false),
            (false, false) => (false, false),
            (_, true) => (false, true),
        };

        if failure_marker_exists || !success_marker_exists {
            if success_marker_exists {
                fs::remove(&success_marker_file_path)?
            }
            if failure_marker_exists {
                fs::remove(&failure_marker_file_path)?
            }
        }

        Ok(Self {
            success_marker_file_path,
            failure_marker_file_path,
            success_before,
            failure_before,
        })
    }

    pub fn is_up_to_date(&self) -> bool {
        !self.failure_before && self.success_before
    }

    pub fn success(&self) -> anyhow::Result<()> {
        fs::write_str(&self.success_marker_file_path, "")
    }

    pub fn failure(&self) -> anyhow::Result<()> {
        fs::write_str(&self.failure_marker_file_path, "")
    }

    pub fn result<T>(&self, result: anyhow::Result<T>) -> anyhow::Result<T> {
        match result {
            Ok(result) => {
                self.success()?;
                Ok(result)
            }
            Err(source_err) => {
                self.failure().with_context(|| {
                    anyhow!(
                        "Failed to save failure marker for source error: {:?}",
                        source_err,
                    )
                })?;
                Err(source_err)
            }
        }
    }
}
