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

use crate::client::check_http_response_success;
use crate::log::{LogColorize, LogIndent, log_action};
use crate::model::app::{CanonicalFilePathWithPermissions, InitialComponentFile};
use anyhow::{Context, anyhow, bail};
use async_trait::async_trait;
use async_zip::tokio::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use golem_common::model::component::ArchiveFilePath;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tokio::fs::File;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReadDirStream;
use url::Url;

#[derive(Debug, Clone)]
struct LoadedFile {
    content: Vec<u8>,
    source: Url,
}

#[derive(Debug, Clone)]
pub struct HashedFile {
    pub hash: blake3::Hash,
    pub target: CanonicalFilePathWithPermissions,
}

#[derive(Debug)]
pub struct ComponentFilesArchive {
    pub archive_path: PathBuf,
    _temp_dir: TempDir, // archive_path is only valid as long as this is alive
}

impl ComponentFilesArchive {
    pub async fn open_archive(&self) -> anyhow::Result<File> {
        File::open(&self.archive_path).await.with_context(|| {
            anyhow!(
                "Failed to open IFS archive: {}",
                self.archive_path.display()
            )
        })
    }
}

pub struct IfsFileManager {
    client: reqwest::Client,
}

fn source_key(source: &Url) -> String {
    source.as_str().to_string()
}

fn archive_file_name_for_source(source: &Url) -> String {
    let file_name = source
        .path_segments()
        .and_then(|segments| segments.filter(|segment| !segment.is_empty()).next_back())
        .unwrap_or("file");

    let sanitized = file_name
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => ch,
            _ => '_',
        })
        .collect::<String>();

    if sanitized.is_empty() {
        "file".to_string()
    } else {
        sanitized
    }
}

fn add_counter_suffix(file_name: &str, counter: usize) -> String {
    if counter <= 1 {
        return file_name.to_string();
    }

    match file_name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
            format!("{stem}-{counter}.{ext}")
        }
        _ => format!("{file_name}-{counter}"),
    }
}

pub fn resolve_archive_paths_for_sources<I>(sources: I) -> anyhow::Result<BTreeMap<String, ArchiveFilePath>>
where
    I: IntoIterator<Item = Url>,
{
    let unique_sources: BTreeSet<Url> = sources.into_iter().collect();
    let mut used_archive_paths: HashSet<String> = HashSet::new();
    let mut result = BTreeMap::new();

    for source in unique_sources {
        let file_name = archive_file_name_for_source(&source);
        let mut counter = 1;

        let archive_path = loop {
            let candidate_name = add_counter_suffix(&file_name, counter);
            let candidate_path = format!("/.golem-ifs/{candidate_name}");

            if used_archive_paths.insert(candidate_path.clone()) {
                break candidate_path;
            }

            counter += 1;
        };

        let archive_path = ArchiveFilePath::from_abs_str(&archive_path)
            .map_err(|err| anyhow!("Invalid generated archive path {archive_path}: {err}"))?;

        result.insert(source_key(&source), archive_path);
    }

    Ok(result)
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

        let temp_dir = tempfile::Builder::new()
            .prefix("golem-cli-zip")
            .tempdir()
            .with_context(|| "Error creating temporary dir for IFS archive")?;
        let archive_path = temp_dir.path().join("data.zip");
        let zip_file = File::create(&archive_path)
            .await
            .with_context(|| "Error creating zip file for IFS archive")?;
        let mut zip_writer = ZipFileWriter::with_tokio(zip_file);

        let mut loaded_files = Vec::new();
        for component_file in component_files {
            loaded_files.extend(
                self.process_component_file(&file_processor, component_file)
                    .await?,
            );
        }

        let archive_path_map = resolve_archive_paths_for_sources(
            loaded_files.iter().map(|file| file.source.clone()),
        )?;

        let mut seen_archive_paths: HashSet<ArchiveFilePath> = HashSet::new();

        for LoadedFile { content, source } in loaded_files {
            let key = source_key(&source);
            let archive_path = archive_path_map
                .get(&key)
                .ok_or_else(|| anyhow!("Missing archive path mapping for source {}", source))?;

            if !seen_archive_paths.insert(archive_path.clone()) {
                continue;
            }

            let zip_entry_name = archive_path.to_rel_string();
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
        }

        zip_writer.close().await.with_context(|| {
            anyhow!(
                "Error closing zip file for IFS archive {}",
                archive_path.display()
            )
        })?;

        Ok(ComponentFilesArchive {
            _temp_dir: temp_dir,
            archive_path,
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
        let mut queue: VecDeque<(PathBuf, CanonicalFilePathWithPermissions)> =
            vec![(source_path, component_file.target.clone())].into();

        while let Some((path, target)) = queue.pop_front() {
            if path.is_dir() {
                let read_dir = tokio::fs::read_dir(&path)
                    .await
                    .with_context(|| anyhow!("Error reading directory: {}", path.display()))?;
                let mut read_dir_stream = ReadDirStream::new(read_dir);
                let mut entries = Vec::new();

                while let Some(entry) = read_dir_stream.next().await {
                    entries.push(entry.context("Error reading directory entry")?);
                }

                entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

                for entry in entries {
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
        target: &CanonicalFilePathWithPermissions,
    ) -> anyhow::Result<R>;

    async fn process_remote_file(
        &self,
        url: &Url,
        target: &CanonicalFilePathWithPermissions,
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
        _target: &CanonicalFilePathWithPermissions,
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

        let source = Url::from_file_path(path)
            .map_err(|_| anyhow!("Failed to convert local IFS file path to URL: {}", path.display()))?;

        Ok(LoadedFile { content, source })
    }

    async fn process_remote_file(
        &self,
        url: &Url,
        _target: &CanonicalFilePathWithPermissions,
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
            source: url.clone(),
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
        target: &CanonicalFilePathWithPermissions,
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
            hash: hasher.finalize(),
            target: target.clone(),
        })
    }

    async fn process_remote_file(
        &self,
        url: &Url,
        target: &CanonicalFilePathWithPermissions,
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
            hash: hasher.finalize(),
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

#[cfg(test)]
mod tests {
    use super::resolve_archive_paths_for_sources;
    use std::collections::BTreeMap;
    use test_r::test;
    use url::Url;

    #[test]
    fn archive_path_is_deterministic_for_same_source_path() {
        let sources = vec![Url::parse("file:///tmp/some/path/main.ts").unwrap()];

        let first = resolve_archive_paths_for_sources(sources.clone()).unwrap();
        let second = resolve_archive_paths_for_sources(sources).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn archive_path_differs_for_different_source_paths() {
        let first_source = Url::parse("file:///tmp/some/path/main.ts").unwrap();
        let second_source = Url::parse("file:///tmp/some/path/other-main.ts").unwrap();

        let paths = resolve_archive_paths_for_sources(vec![first_source.clone(), second_source.clone()])
            .unwrap();

        let first = paths.get(first_source.as_str()).unwrap();
        let second = paths.get(second_source.as_str()).unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn archive_path_uses_counter_on_name_collision() {
        let first_source = Url::parse("file:///tmp/a/main.ts").unwrap();
        let second_source = Url::parse("file:///tmp/b/main.ts").unwrap();

        let paths: BTreeMap<String, _> =
            resolve_archive_paths_for_sources(vec![first_source.clone(), second_source.clone()])
                .unwrap();

        let first = paths.get(first_source.as_str()).unwrap().to_string();
        let second = paths.get(second_source.as_str()).unwrap().to_string();

        assert_ne!(first, second);
        assert!(
            (first.ends_with("/.golem-ifs/main.ts") && second.ends_with("/.golem-ifs/main-2.ts"))
                || (second.ends_with("/.golem-ifs/main.ts")
                    && first.ends_with("/.golem-ifs/main-2.ts"))
        );
    }
}
