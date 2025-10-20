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

use super::ComponentError;
use crate::model::component::{
    ConflictReport, ConflictingFunction, ParameterTypeConflict, ReturnTypeConflict,
};
use anyhow::anyhow;
use async_zip::ZipEntry;
use async_zip::tokio::read::seek::ZipFileReader;
use futures::TryStreamExt;
use golem_common::model::component::ComponentFilePath;
use golem_common::model::component_constraint::FunctionConstraints;
use golem_service_base::replayable_stream::ReplayableStream;
use golem_wasm::analysis::AnalysedType;
use rib::FunctionDictionary;
use std::sync::Arc;
use std::vec;
use tempfile::NamedTempFile;
use tokio::io::BufReader;
use tokio_stream::Stream;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tokio_util::io::ReaderStream;
use tracing::info;
use wac_graph::types::Package;
use wac_graph::{CompositionGraph, EncodeOptions, PlugError};

pub async fn prepare_component_files_for_upload(
    archive: NamedTempFile,
) -> Result<Vec<(ComponentFilePath, ZipEntryStream)>, ComponentError> {
    let archive = Arc::new(archive);
    let archive_clone = archive.clone();
    let reopened = tokio::task::spawn_blocking(move || archive_clone.reopen())
        .await
        .map_err(anyhow::Error::from)?
        .map_err(anyhow::Error::from)?;

    let mut buf_reader = BufReader::new(tokio::fs::File::from_std(reopened));

    let mut zip_archive = ZipFileReader::with_tokio(&mut buf_reader)
        .await
        .map_err(anyhow::Error::from)?;

    let mut result = vec![];

    for i in 0..zip_archive.file().entries().len() {
        let entry_reader = zip_archive
            .reader_with_entry(i)
            .await
            .map_err(anyhow::Error::from)?;

        let entry = entry_reader.entry();

        let is_dir = entry.dir().map_err(anyhow::Error::from)?;

        if is_dir {
            continue;
        }

        let path = initial_component_file_path_from_zip_entry(entry)?;

        let stream = ZipEntryStream::from_zip_file_and_index(archive.clone(), i);

        result.push((path, stream));
    }

    Ok(result)
}

pub struct ZipEntryStream {
    file: Arc<NamedTempFile>,
    index: usize,
}

impl ZipEntryStream {
    pub fn from_zip_file_and_index(file: Arc<NamedTempFile>, index: usize) -> Self {
        Self { file, index }
    }
}

impl ReplayableStream for ZipEntryStream {
    type Item = Result<Vec<u8>, anyhow::Error>;
    type Error = anyhow::Error;

    async fn make_stream(
        &self,
    ) -> Result<impl Stream<Item = Self::Item> + Send + 'static, Self::Error> {
        let file = self.file.clone();
        let reopened = tokio::task::spawn_blocking(move || file.reopen()).await??;
        let buf_reader = BufReader::new(tokio::fs::File::from_std(reopened));
        let zip_archive = ZipFileReader::with_tokio(buf_reader).await?;
        let entry_reader = zip_archive.into_entry(self.index).await?;
        let stream = ReaderStream::new(entry_reader.compat());
        let mapped_stream = stream.map_ok(|b| b.to_vec()).map_err(|e| e.into());
        Ok(Box::pin(mapped_stream))
    }

    async fn length(&self) -> Result<u64, Self::Error> {
        let file = self.file.clone();
        let reopened = tokio::task::spawn_blocking(move || file.reopen()).await??;
        let buf_reader = BufReader::new(tokio::fs::File::from_std(reopened));
        let zip_archive = ZipFileReader::with_tokio(buf_reader).await?;

        Ok(zip_archive
            .file()
            .entries()
            .get(self.index)
            .ok_or(anyhow!("Entry with not found in archive"))?
            .uncompressed_size())
    }
}

fn initial_component_file_path_from_zip_entry(
    entry: &ZipEntry,
) -> Result<ComponentFilePath, ComponentError> {
    let file_path =
        entry
            .filename()
            .as_str()
            .map_err(|e| ComponentError::MalformedComponentArchive {
                message: format!("Failed to convert filename to string: {e}"),
            })?;

    // convert windows path separators to unix and sanitize the path
    let file_path: String = file_path
        .replace('\\', "/")
        .split('/')
        .map(sanitize_filename::sanitize)
        .collect::<Vec<_>>()
        .join("/");

    ComponentFilePath::from_abs_str(&format!("/{file_path}")).map_err(|e| {
        ComponentError::MalformedComponentArchive {
            message: format!("Failed to convert path to InitialComponentFilePath: {e}"),
        }
    })
}

pub fn _find_component_metadata_conflicts(
    function_constraints: &FunctionConstraints,
    new_type_registry: &FunctionDictionary,
) -> ConflictReport {
    let mut missing_functions = vec![];
    let mut conflicting_functions = vec![];

    for existing_function_call in &function_constraints.constraints {
        if let Some(new_registry_value) =
            new_type_registry.get(existing_function_call.function_key())
        {
            let mut parameter_conflict = false;
            let mut return_conflict = false;

            if existing_function_call.parameter_types() != &new_registry_value.parameter_types() {
                parameter_conflict = true;
            }

            let new_return_type = new_registry_value
                .return_type
                .as_ref()
                .map(|x| AnalysedType::try_from(x).unwrap());

            // AnalysedType conversion from function `FunctionType` should never fail
            if existing_function_call.return_type() != &new_return_type {
                return_conflict = true;
            }

            let parameter_conflict = if parameter_conflict {
                Some(ParameterTypeConflict {
                    existing: existing_function_call.parameter_types().clone(),
                    new: new_registry_value.clone().parameter_types().clone(),
                })
            } else {
                None
            };

            let return_conflict = if return_conflict {
                Some(ReturnTypeConflict {
                    existing: existing_function_call.return_type().clone(),
                    new: new_return_type,
                })
            } else {
                None
            };

            if parameter_conflict.is_some() || return_conflict.is_some() {
                conflicting_functions.push(ConflictingFunction {
                    function: existing_function_call.function_key().clone(),
                    parameter_type_conflict: parameter_conflict,
                    return_type_conflict: return_conflict,
                });
            }
        } else {
            missing_functions.push(existing_function_call.function_key().clone());
        }
    }

    ConflictReport {
        missing_functions,
        conflicting_functions,
    }
}

pub fn compose_components(socket_bytes: &[u8], plug_bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut graph = CompositionGraph::new();

    let socket = Package::from_bytes("socket", None, socket_bytes, graph.types_mut())?;
    let socket = graph.register_package(socket)?;

    let plug_package = Package::from_bytes("plug", None, plug_bytes, graph.types_mut())?;
    let plub_package_id = graph.register_package(plug_package)?;

    match wac_graph::plug(&mut graph, vec![plub_package_id], socket) {
        Ok(()) => {
            let bytes = graph.encode(EncodeOptions::default())?;
            Ok(bytes)
        }
        Err(PlugError::NoPlugHappened) => {
            info!("No plugs where executed when composing components");
            Ok(socket_bytes.to_vec())
        }
        Err(error) => Err(error.into()),
    }
}
