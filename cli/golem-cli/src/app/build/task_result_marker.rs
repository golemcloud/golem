// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::app::build::task_result_marker::TaskResultMarkerHashSourceKind::{Hash, HashFromString};
use crate::fs;
use crate::log::log_warn_action;
use crate::model::app::{AppComponentName, DependentComponent};
use crate::model::app_raw::{GenerateQuickJSCrate, GenerateQuickJSDTS};
use crate::model::ProjectId;
use crate::model::{app_raw, ComponentName};
use anyhow::{anyhow, bail, Context};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use wit_parser::PackageName;

pub enum TaskResultMarkerHashSourceKind {
    // The string will be hashed
    HashFromString(String),
    // The string will be used as the hash, expected to be in hex format
    Hash(String),
}

pub trait TaskResultMarkerHashSource {
    fn kind() -> &'static str;

    /// The hashed value of id will be used as the task result marker filename.
    ///
    /// If id() returns None, then the source will be used as id.
    ///
    /// Specifying the id is optional, as some tasks are their own identity, like external commands.
    /// In those cases we can skip calculating values and hashes twice.
    ///
    /// The main difference between id and hash is that it should not include
    /// generic "task properties", only ids for the task. E.g.: the hash_input for rpc linking
    /// should contain all the main and dependency component names and types, while the id should
    /// only contain the main component name which the dependencies are linked into.
    fn id(&self) -> anyhow::Result<Option<String>>;

    /// The source will be used for calculating the hash value for the task result marker.
    /// It should contain all the properties of the task which should trigger re-runs.
    /// Note that currently we usually do not include file sources in these, as for those
    /// we use mod-time based checks together with task markers.
    fn source(&self) -> anyhow::Result<TaskResultMarkerHashSourceKind>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResult {
    // NOTE: kind is optional, only used for debugging
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    // NOTE: id is optional, only used for debugging
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    // NOTE: hash_input is optional, only used for debugging
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_input: Option<String>,

    pub hash_hex: String,
    pub success: bool,
}

#[derive(Serialize)]
pub struct ResolvedExternalCommandMarkerHash<'a> {
    pub build_dir: &'a Path,
    pub command: &'a app_raw::ExternalCommand,
}

impl TaskResultMarkerHashSource for ResolvedExternalCommandMarkerHash<'_> {
    fn kind() -> &'static str {
        "ResolvedExternalCommandMarkerHash"
    }

    fn id(&self) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    fn source(&self) -> anyhow::Result<TaskResultMarkerHashSourceKind> {
        Ok(HashFromString(serde_json::to_string(self)?))
    }
}

#[derive(Serialize)]
pub struct GenerateQuickJSCrateCommandMarkerHash<'a> {
    pub build_dir: &'a Path,
    pub command: &'a GenerateQuickJSCrate,
}

impl TaskResultMarkerHashSource for GenerateQuickJSCrateCommandMarkerHash<'_> {
    fn kind() -> &'static str {
        "GenerateQuickJSCrateCommandMarkerHash"
    }

    fn id(&self) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    fn source(&self) -> anyhow::Result<TaskResultMarkerHashSourceKind> {
        Ok(HashFromString(serde_json::to_string(self)?))
    }
}

#[derive(Serialize)]
pub struct GenerateQuickJSDTSCommandMarkerHash<'a> {
    pub build_dir: &'a Path,
    pub command: &'a GenerateQuickJSDTS,
}

impl TaskResultMarkerHashSource for GenerateQuickJSDTSCommandMarkerHash<'_> {
    fn kind() -> &'static str {
        "GenerateQuickJSDTSCommandMarkerHash"
    }

    fn id(&self) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    fn source(&self) -> anyhow::Result<TaskResultMarkerHashSourceKind> {
        Ok(HashFromString(serde_json::to_string(self)?))
    }
}

pub struct ComponentGeneratorMarkerHash<'a> {
    pub component_name: &'a AppComponentName,
    pub generator_kind: &'a str,
}

impl TaskResultMarkerHashSource for ComponentGeneratorMarkerHash<'_> {
    fn kind() -> &'static str {
        "ComponentGeneratorMarkerHash"
    }

    fn id(&self) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    fn source(&self) -> anyhow::Result<TaskResultMarkerHashSourceKind> {
        Ok(HashFromString(format!(
            "{}-{}",
            self.component_name, self.generator_kind
        )))
    }
}

pub struct LinkRpcMarkerHash<'a> {
    pub component_name: &'a AppComponentName,
    pub dependencies: &'a BTreeSet<&'a DependentComponent>,
}

impl TaskResultMarkerHashSource for LinkRpcMarkerHash<'_> {
    fn kind() -> &'static str {
        "RpcLinkMarkerHash"
    }

    fn id(&self) -> anyhow::Result<Option<String>> {
        Ok(Some(self.component_name.to_string()))
    }

    fn source(&self) -> anyhow::Result<TaskResultMarkerHashSourceKind> {
        Ok(HashFromString(format!(
            "{}#{}",
            self.component_name,
            self.dependencies
                .iter()
                .map(|s| format!("{}#{}", s.source, s.dep_type.as_str()))
                .join(",")
        )))
    }
}

pub struct AddMetadataMarkerHash<'a> {
    pub component_name: &'a AppComponentName,
    pub root_package_name: PackageName,
}

impl TaskResultMarkerHashSource for AddMetadataMarkerHash<'_> {
    fn kind() -> &'static str {
        "AddMetadataMarkerHash"
    }

    fn id(&self) -> anyhow::Result<Option<String>> {
        Ok(Some(self.component_name.to_string()))
    }

    fn source(&self) -> anyhow::Result<TaskResultMarkerHashSourceKind> {
        Ok(HashFromString(self.root_package_name.to_string()))
    }
}

pub struct GetServerComponentHash<'a> {
    pub project_id: Option<&'a ProjectId>,
    pub component_name: &'a ComponentName,
    pub component_version: u64,
    // NOTE: use None for querying
    pub component_hash: Option<&'a str>,
}

impl TaskResultMarkerHashSource for GetServerComponentHash<'_> {
    fn kind() -> &'static str {
        "GetServerComponentHash"
    }

    fn id(&self) -> anyhow::Result<Option<String>> {
        Ok(Some(format!(
            "{:?}#{}#{}",
            self.project_id, self.component_name, self.component_version
        )))
    }

    fn source(&self) -> anyhow::Result<TaskResultMarkerHashSourceKind> {
        match self.component_hash {
            Some(hash) => Ok(Hash(hash.to_string())),
            None => bail!("Missing precalculated hash for {}", self.component_name),
        }
    }
}

pub struct GetServerIfsFileHash<'a> {
    pub project_id: Option<&'a ProjectId>,
    pub component_name: &'a ComponentName,
    pub component_version: u64,
    pub target_path: &'a str,
    // NOTE: use None for querying
    pub file_hash: Option<&'a str>,
}

impl TaskResultMarkerHashSource for GetServerIfsFileHash<'_> {
    fn kind() -> &'static str {
        "GetServerIfsFileHash"
    }

    fn id(&self) -> anyhow::Result<Option<String>> {
        Ok(Some(format!(
            "{:?}#{}#{}#{}",
            self.project_id, self.component_name, self.component_version, self.target_path
        )))
    }

    fn source(&self) -> anyhow::Result<TaskResultMarkerHashSourceKind> {
        match self.file_hash {
            Some(hash) => Ok(Hash(hash.to_string())),
            None => bail!(
                "Missing precalculated hash for {} - {}",
                self.component_name,
                self.target_path
            ),
        }
    }
}

pub struct TaskResultMarker {
    kind: &'static str,
    id: String,
    hash_input: String,
    marker_file_path: PathBuf,
    hash_hex: String,
    previous_result: Option<TaskResult>,
}

impl TaskResultMarker {
    pub fn new<T: TaskResultMarkerHashSource>(dir: &Path, task: T) -> anyhow::Result<Self> {
        let (hash_input, hash_hex) = match task.source()? {
            HashFromString(hash_input) => {
                let mut hasher = blake3::Hasher::new();
                hasher.update(hash_input.as_bytes());
                (hash_input, hasher.finalize().to_hex().to_string())
            }
            Hash(hash) => (hash.clone(), hash),
        };

        let (id_hash_hex, id) = {
            match task.id()? {
                Some(id) => (Self::id_hash_hex::<T>(&id), id),
                None => (hash_hex.clone(), hash_input.clone()),
            }
        };

        let (marker_file_path, marker_file_exists, previous_result) =
            Self::load_previous_result(dir, &id_hash_hex)?;

        let task_result_marker = Self {
            kind: T::kind(),
            id,
            hash_input,
            marker_file_path,
            hash_hex,
            previous_result,
        };

        if marker_file_exists && !task_result_marker.is_up_to_date() {
            fs::remove(&task_result_marker.marker_file_path)?;
        }

        Ok(task_result_marker)
    }

    pub fn get_hash<T: TaskResultMarkerHashSource>(
        dir: &Path,
        task: T,
    ) -> anyhow::Result<Option<String>> {
        let id_hash_hex = {
            match task.id()? {
                Some(id) => Self::id_hash_hex::<T>(&id),
                None => bail!("missing id for get_hash, task kind: {}", T::kind()),
            }
        };

        let (_marker_file_path, _marker_file_exists, previous_result) =
            Self::load_previous_result(dir, &id_hash_hex)?;

        Ok(previous_result.map(|previous_result| previous_result.hash_hex))
    }

    fn id_hash_hex<T: TaskResultMarkerHashSource>(id: &str) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(T::kind().as_bytes());
        hasher.update(id.as_bytes());
        hasher.finalize().to_hex().to_string()
    }

    fn load_previous_result(
        dir: &Path,
        id_hash_hex: &str,
    ) -> anyhow::Result<(PathBuf, bool, Option<TaskResult>)> {
        let marker_file_path = dir.join(id_hash_hex);
        let marker_file_exists = marker_file_path.exists();

        let previous_result = {
            if marker_file_exists {
                match serde_json::from_str::<TaskResult>(&fs::read_to_string(&marker_file_path)?) {
                    Ok(result) => Some(result),
                    Err(err) => {
                        log_warn_action(
                            "Ignoring",
                            format!(
                                "invalid task marker {}: {}",
                                marker_file_path.display(),
                                err
                            ),
                        );
                        None
                    }
                }
            } else {
                None
            }
        };

        Ok((marker_file_path, marker_file_exists, previous_result))
    }

    pub fn is_up_to_date(&self) -> bool {
        match &self.previous_result {
            Some(previous_result) => {
                previous_result.hash_hex == self.hash_hex && previous_result.success
            }
            None => false,
        }
    }

    pub fn success(self) -> anyhow::Result<()> {
        self.save_marker_file(true)
    }

    pub fn failure(self) -> anyhow::Result<()> {
        self.save_marker_file(false)
    }

    fn save_marker_file(self, success: bool) -> anyhow::Result<()> {
        fs::write_str(
            &self.marker_file_path,
            &serde_json::to_string_pretty(&TaskResult {
                // TODO: setting kind, id and hash_input could be driven by a debug flag, env or build
                kind: Some(self.kind.to_string()),
                id: Some(self.id),
                hash_input: Some(self.hash_input),
                hash_hex: self.hash_hex,
                success,
            })?,
        )
    }

    pub fn result<T>(self, result: anyhow::Result<T>) -> anyhow::Result<T> {
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
