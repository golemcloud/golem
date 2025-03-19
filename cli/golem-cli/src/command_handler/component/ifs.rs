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

use crate::model::app_ext::InitialComponentFile;
use anyhow::{anyhow, bail, Context};
use async_zip::tokio::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use golem_common::model::{
    ComponentFilePath, ComponentFilePathWithPermissions, ComponentFilePathWithPermissionsList,
};
use golem_wasm_rpc_stubgen::log::{log_action, LogColorize, LogIndent};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::fs::File;
use tokio_stream::wrappers::ReadDirStream;
use tokio_stream::StreamExt;

#[derive(Debug, Clone)]
pub struct LoadedFile {
    content: Vec<u8>,
    target: ComponentFilePathWithPermissions,
}

#[derive(Debug)]
pub struct ComponentFilesArchive {
    pub archive_path: PathBuf,
    pub properties: ComponentFilePathWithPermissionsList,
    _temp_dir: TempDir, // archive_path is only valid as long as this is alive
}

pub struct IfsArchiveBuilder {
    client: reqwest::Client,
}

impl IfsArchiveBuilder {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    pub async fn build_files_archive(
        &self,
        component_files: Vec<InitialComponentFile>,
    ) -> anyhow::Result<ComponentFilesArchive> {
        log_action("Creating", "IFS archive");
        let _indent = LogIndent::new();

        let temp_dir = tempfile::Builder::new()
            .prefix("golem-cli-zip")
            .tempdir()
            .with_context(|| "Error creating temporary dir for IFS archive")?;
        let zip_file_path = temp_dir.path().join("data.zip");
        let zip_file = File::create(&zip_file_path)
            .await
            .with_context(|| "Error creating zip file for IFS archive")?;

        let mut zip_writer = ZipFileWriter::with_tokio(zip_file);

        let mut seen_paths: HashSet<ComponentFilePath> = HashSet::new();
        let mut successfully_added: Vec<ComponentFilePathWithPermissions> = vec![];

        for component_file in component_files {
            for LoadedFile { content, target } in self.load_file(component_file).await? {
                if !seen_paths.insert(target.path.clone()) {
                    bail!("Conflicting paths in component files: {}", target.path);
                }

                // zip files do not like absolute paths. Convert the absolute component path to relative path from the root of the zip file
                let zip_entry_name = target.path.to_string();
                let builder =
                    ZipEntryBuilder::new(zip_entry_name.clone().into(), Compression::Deflate);

                log_action(
                    "Adding",
                    format!(
                        "entry {} to IFS archive",
                        zip_entry_name.log_color_highlight()
                    ),
                );

                zip_writer
                    .write_entry_whole(builder, &content)
                    .await
                    .with_context(|| {
                        anyhow!("Error writing zip entry for IFS archive {}", zip_entry_name)
                    })?;

                successfully_added.push(target);
            }
        }
        zip_writer.close().await.with_context(|| {
            anyhow!(
                "Error closing zip file for IFS archive {}",
                zip_file_path.display()
            )
        })?;

        let properties = ComponentFilePathWithPermissionsList {
            values: successfully_added,
        };

        Ok(ComponentFilesArchive {
            _temp_dir: temp_dir,
            archive_path: zip_file_path,
            properties,
        })
    }

    async fn load_file(
        &self,
        component_file: InitialComponentFile,
    ) -> anyhow::Result<Vec<LoadedFile>> {
        let scheme = component_file.source_path.as_url().scheme();
        match scheme {
            "file" | "" => self.load_local_file(component_file).await,
            "http" | "https" => self
                .download_remote_file(component_file)
                .await
                .map(|f| vec![f]),
            _ => Err(anyhow!(
                "Unsupported scheme '{}' for IFS file: {}",
                scheme,
                component_file.source_path.as_url()
            )),
        }
    }

    async fn load_local_file(
        &self,
        component_file: InitialComponentFile,
    ) -> anyhow::Result<Vec<LoadedFile>> {
        // if it's a directory, we need to recursively load all files and combine them with their target paths and permissions.
        let source_path = PathBuf::from(component_file.source_path.as_url().path());

        let mut results: Vec<LoadedFile> = vec![];
        let mut queue: VecDeque<(PathBuf, ComponentFilePathWithPermissions)> =
            vec![(source_path, component_file.target)].into();

        while let Some((path, target)) = queue.pop_front() {
            if path.is_dir() {
                let read_dir = tokio::fs::read_dir(&path)
                    .await
                    .with_context(|| anyhow!("Error reading directory: {}", path.display()))?;
                let mut read_dir_stream = ReadDirStream::new(read_dir);

                while let Some(entry) = read_dir_stream.next().await {
                    let entry = entry.context("Error reading directory entry")?;
                    let next_path = entry.path();

                    let file_name = entry.file_name().into_string().map_err(|_| {
                        anyhow!(
                            "Error converting file name to string: Contains non-unicode data"
                                .to_string(),
                        )
                    })?;

                    let mut new_target = target.clone();
                    new_target
                        .extend_path(file_name.as_str())
                        .map_err(|err| anyhow!("Error extending path: {err}"))?;

                    queue.push_back((next_path, new_target));
                }
            } else {
                log_action(
                    "Loading",
                    format!(
                        "local file: {}",
                        path.display().to_string().log_color_highlight()
                    ),
                );
                let content = tokio::fs::read(&path)
                    .await
                    .with_context(|| anyhow!("Error reading component file: {}", path.display()))?;
                results.push(LoadedFile { content, target });
            }
        }
        Ok(results)
    }

    async fn download_remote_file(
        &self,
        component_file: InitialComponentFile,
    ) -> anyhow::Result<LoadedFile> {
        let url = component_file.source_path.into_url();
        log_action(
            "Downloading",
            format!("remote file: {}", url.as_str().log_color_highlight()),
        );
        let response = self
            .client
            .get(url.clone())
            .send()
            .await
            .with_context(|| anyhow!("Failed to download IFS file: {}", url))?;
        let bytes = response.bytes().await?;
        let content = bytes.to_vec();

        Ok(LoadedFile {
            content,
            target: component_file.target,
        })
    }
}
