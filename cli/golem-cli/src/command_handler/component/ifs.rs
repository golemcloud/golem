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

use crate::context::check_http_response_success;
use crate::log::{log_action, LogColorize, LogIndent};
use crate::model::app::InitialComponentFile;
use anyhow::{anyhow, bail, Context};
use async_trait::async_trait;
use async_zip::tokio::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use golem_common::model::{ComponentFilePathWithPermissions, ComponentFilePathWithPermissionsList};
use itertools::Itertools;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tokio::fs::File;
use tokio_stream::wrappers::ReadDirStream;
use tokio_stream::StreamExt;
use url::Url;

#[derive(Debug, Clone)]
struct LoadedFile {
    content: Vec<u8>,
    target: ComponentFilePathWithPermissions,
}

#[derive(Debug, Clone)]
pub struct HashedFile {
    pub hash_hex: String,
    pub target: ComponentFilePathWithPermissions,
}

#[derive(Debug)]
pub struct ComponentFilesArchive {
    pub archive_path: PathBuf,
    pub properties: ComponentFilePathWithPermissionsList,
    _temp_dir: TempDir, // archive_path is only valid as long as this is alive
}

pub struct IfsFileManager {
    client: reqwest::Client,
}

impl IfsFileManager {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    pub async fn build_files_archive(
        &self,
        component_files: &[InitialComponentFile],
    ) -> anyhow::Result<ComponentFilesArchive> {
        let file_processor = FileLoader {
            client: self.client.clone(),
        };

        log_action("Creating", "IFS archive");
        let _indent = LogIndent::new();

        validate_unique_targets(component_files)?;

        let temp_dir = tempfile::Builder::new()
            .prefix("golem-cli-zip")
            .tempdir()
            .with_context(|| "Error creating temporary dir for IFS archive")?;
        let zip_file_path = temp_dir.path().join("data.zip");
        let zip_file = File::create(&zip_file_path)
            .await
            .with_context(|| "Error creating zip file for IFS archive")?;
        let mut zip_writer = ZipFileWriter::with_tokio(zip_file);

        let mut successfully_added: Vec<ComponentFilePathWithPermissions> =
            Vec::with_capacity(component_files.len());

        for component_file in component_files {
            for LoadedFile { content, target } in self
                .process_component_file(&file_processor, component_file)
                .await?
            {
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

    pub async fn collect_file_hashes(
        &self,
        component_name: &str,
        component_files: &[InitialComponentFile],
    ) -> anyhow::Result<Vec<HashedFile>> {
        if component_files.is_empty() {
            return Ok(Vec::new());
        }

        let file_processor = FileHasher {
            client: self.client.clone(),
        };

        log_action(
            "Calculating hashes",
            format!(
                "for manifest IFS files, component: {}",
                component_name.log_color_highlight()
            ),
        );
        let _indent = LogIndent::new();

        validate_unique_targets(component_files)?;

        let mut hashes = Vec::with_capacity(component_files.len());
        for component_file in component_files {
            hashes.extend(
                self.process_component_file(&file_processor, component_file)
                    .await?,
            );
        }

        Ok(hashes)
    }

    async fn process_component_file<R>(
        &self,
        file_processor: &dyn FileProcessor<R>,
        component_file: &InitialComponentFile,
    ) -> anyhow::Result<Vec<R>> {
        let scheme = component_file.source.as_url().scheme();
        match scheme {
            "file" | "" => {
                self.process_local_path(file_processor, component_file)
                    .await
            }
            "http" | "https" => self
                .process_remote_file(file_processor, component_file)
                .await
                .map(|f| vec![f]),
            _ => Err(anyhow!(
                "Unsupported scheme '{}' for IFS file: {}",
                scheme,
                component_file.source.as_url()
            )),
        }
    }

    async fn process_local_path<R>(
        &self,
        file_processor: &dyn FileProcessor<R>,
        component_file: &InitialComponentFile,
    ) -> anyhow::Result<Vec<R>> {
        // if it's a directory, we need to recursively load all files and combine them with their target paths and permissions.
        let source_path = PathBuf::from(component_file.source.as_url().path());

        let mut results: Vec<R> = vec![];
        let mut queue: VecDeque<(PathBuf, ComponentFilePathWithPermissions)> =
            vec![(source_path, component_file.target.clone())].into();

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
                results.push(file_processor.process_local_file(&path, &target).await?);
            }
        }
        Ok(results)
    }

    async fn process_remote_file<R>(
        &self,
        file_processor: &dyn FileProcessor<R>,
        component_file: &InitialComponentFile,
    ) -> anyhow::Result<R> {
        file_processor
            .process_remote_file(component_file.source.as_url(), &component_file.target)
            .await
    }
}

#[async_trait]
trait FileProcessor<R> {
    async fn process_local_file(
        &self,
        path: &Path,
        target: &ComponentFilePathWithPermissions,
    ) -> anyhow::Result<R>;

    async fn process_remote_file(
        &self,
        url: &Url,
        target: &ComponentFilePathWithPermissions,
    ) -> anyhow::Result<R>;
}

struct FileLoader {
    client: reqwest::Client,
}

#[async_trait]
impl FileProcessor<LoadedFile> for FileLoader {
    async fn process_local_file(
        &self,
        path: &Path,
        target: &ComponentFilePathWithPermissions,
    ) -> anyhow::Result<LoadedFile> {
        log_action(
            "Loading",
            format!(
                "local IFS file: {}",
                path.display().to_string().log_color_highlight()
            ),
        );

        let content = tokio::fs::read(&path)
            .await
            .with_context(|| anyhow!("Error reading local IFS file: {}", path.display()))?;

        Ok(LoadedFile {
            content,
            target: target.clone(),
        })
    }

    async fn process_remote_file(
        &self,
        url: &Url,
        target: &ComponentFilePathWithPermissions,
    ) -> anyhow::Result<LoadedFile> {
        log_action(
            "Downloading",
            format!("remote IFS file: {}", url.as_str().log_color_highlight()),
        );

        let response = self
            .client
            .get(url.clone())
            .send()
            .await
            .with_context(|| anyhow!("Failed to download remote IFS file: {}", url))?;

        let response = check_http_response_success(response).await?;

        let bytes = response
            .bytes()
            .await
            .with_context(|| anyhow!("Failed to download remote IFS file: {}", url))?;

        Ok(LoadedFile {
            content: bytes.into(),
            target: target.clone(),
        })
    }
}

struct FileHasher {
    client: reqwest::Client,
}

#[async_trait]
impl FileProcessor<HashedFile> for FileHasher {
    async fn process_local_file(
        &self,
        path: &Path,
        target: &ComponentFilePathWithPermissions,
    ) -> anyhow::Result<HashedFile> {
        log_action(
            "Calculating hash",
            format!(
                "for local IFS file: {}",
                path.display().to_string().log_color_highlight()
            ),
        );

        let mut hasher = blake3::Hasher::new();
        hasher
            .update_reader(
                std::fs::File::open(path)
                    .with_context(|| anyhow!("Error reading local IFS file: {}", path.display()))?,
            )
            .with_context(|| anyhow!("Failed to hash local IFS file: {}", path.display()))?;

        Ok(HashedFile {
            hash_hex: hasher.finalize().to_hex().to_string(),
            target: target.clone(),
        })
    }

    async fn process_remote_file(
        &self,
        url: &Url,
        target: &ComponentFilePathWithPermissions,
    ) -> anyhow::Result<HashedFile> {
        log_action(
            "Calculating hash",
            format!(
                "for remote IFS file: {}",
                url.as_str().log_color_highlight()
            ),
        );
        let response = self
            .client
            .get(url.clone())
            .send()
            .await
            .with_context(|| anyhow!("Failed to stream remote IFS file: {}", url))?;

        let response = check_http_response_success(response).await?;

        let mut hasher = blake3::Hasher::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let bytes =
                chunk.with_context(|| anyhow!("Failed to stream remote IFS file: {}", url))?;
            hasher.update(&bytes);
        }

        Ok(HashedFile {
            hash_hex: hasher.finalize().to_hex().to_string(),
            target: target.clone(),
        })
    }
}

// TODO: add this to manifest validation (too or instead of doing it here)?
fn validate_unique_targets(component_files: &[InitialComponentFile]) -> anyhow::Result<()> {
    let non_unique_target_paths = component_files
        .iter()
        .map(|file| file.target.path.as_path())
        .counts()
        .into_iter()
        .filter(|&(_, count)| count > 1)
        .map(|(path, _)| path)
        .collect::<Vec<_>>();

    if !non_unique_target_paths.is_empty() {
        bail!(
            "Found duplicated IFS targets: {}",
            non_unique_target_paths.into_iter().join(", ")
        );
    }

    Ok(())
}
