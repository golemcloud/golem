// Copyright 2024 Golem Cloud
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

use crate::async_zip_writer::AsyncZipWriter;
use crate::model::GolemError;
use crate::temp_dir::TempDir;
use golem_common::log::log_action;
use golem_common::model::application::{
    InitialFile, InitialFilePermissions, Resource, WasmComponent,
};
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use tokio::task::JoinSet;
use tokio_stream::StreamExt;
use tokio_util::io::ReaderStream;
use url::Url;
use walkdir::WalkDir;

pub async fn create_archive(
    wasm_component: WasmComponent,
    temp_dir: &TempDir,
) -> Result<Option<(File, PathBuf)>, GolemError> {
    let WasmComponent {
        name,
        source,
        files,
        ..
    } = wasm_component;
    if files.is_empty() {
        return Ok(None);
    }
    log_action(
        "Downloading and packing",
        format!("files for component: {name}"),
    );

    let (downloaded_files_path, zip_writer, archive_path) =
        create_download_dir_and_zip_writer(temp_dir, &name)
            .await
            .map_err(|error| format!("Failed to create empty download dir or archive: {error}"))?;

    let mut file_tasks = files
        .into_iter()
        .enumerate()
        .map({
            let client = Client::new();
            let manifest_dir = Arc::new(
                source
                    .parent()
                    .ok_or_else(|| {
                        format!(
                            "Failed to get manifest dir. Manifest path: '{}'",
                            source.display()
                        )
                    })?
                    .to_path_buf(),
            );
            let zip_writer = zip_writer.clone();
            move |(index, initial_file)| {
                download_and_pack_initial_file(
                    index,
                    initial_file,
                    client.clone(),
                    zip_writer.clone(),
                    Arc::clone(&manifest_dir),
                    Arc::clone(&downloaded_files_path),
                )
            }
        })
        .collect::<JoinSet<_>>();

    while let Some(result) = file_tasks.join_next().await {
        result??
    }

    let finished_archive_file = zip_writer
        .lock()
        .await?
        .finish()
        .await
        .map_err(|error| format!("Failed to finish archive: {error}"))?;

    log_action(
        "Downloaded and packed",
        format!("files for component: {name}"),
    );
    Ok(Some((finished_archive_file, archive_path)))
}

async fn create_download_dir_and_zip_writer(
    temp_dir: &TempDir,
    component_name: &str,
) -> Result<(Arc<PathBuf>, AsyncZipWriter, PathBuf), GolemError> {
    let wasm_component_path = {
        let mut path = PathBuf::new();
        path.push(temp_dir.path());
        path.push("wasm_components");
        path.push(component_name);
        path
    };
    let downloaded_files_path = Arc::new(wasm_component_path.join("downloaded_files"));
    fs::create_dir_all(downloaded_files_path.as_ref()).await?;

    let archive_path = wasm_component_path.join(format!("{component_name}_initial_fs.zip"));
    let archive_file = File::create_new(&archive_path).await?;
    let zip_writer = AsyncZipWriter::new(archive_file).await;

    Ok((downloaded_files_path, zip_writer, archive_path))
}

async fn download_and_pack_initial_file(
    index: usize,
    file: InitialFile,
    client: Client,
    zip_writer: AsyncZipWriter,
    manifest_dir: Arc<PathBuf>,
    downloaded_files_path: Arc<PathBuf>,
) -> Result<(), GolemError> {
    let InitialFile {
        source_path,
        target_path,
        permissions,
    } = file;

    let resolved_source_path = match &source_path {
        Resource::Path(path) => {
            let path = manifest_dir.join(path);
            path.canonicalize().map_err(|error| {
                let path = path.display();
                format!("Failed to resolve path '{path}': {error}")
            })?
        }
        Resource::Url(url) => {
            let path = downloaded_files_path.join(format!("file_{index}"));
            download_file(&client, url, &path)
                .await
                .map_err(|error| format!("Failed to download file from '{url}': {error}"))?;
            path
        }
    };

    let permissions = permissions.unwrap_or_default();
    add_files_to_archive(resolved_source_path, target_path, permissions, zip_writer)
        .await
        .map_err(|error| format!("Failed to pack file(s): {error}"))?;

    log_action("Packed", {
        let permissions = match permissions {
            InitialFilePermissions::ReadOnly => "read-only",
            InitialFilePermissions::ReadWrite => "read-write",
        };
        let source_path = match &source_path {
            Resource::Path(path) => path.to_string_lossy(),
            Resource::Url(url) => url.as_str().into(),
        };
        format!("{permissions} file(s): {source_path}")
    });
    Ok(())
}

async fn download_file(client: &Client, url: &Url, path: &Path) -> Result<(), GolemError> {
    let mut file = File::create_new(path).await?;

    let mut bytes_stream = client.get(url.clone()).send().await?.bytes_stream();

    while let Some(chunk) = bytes_stream.next().await {
        file.write_all(&chunk?).await?;
    }

    file.flush().await?;
    log_action("Downloaded", format!("file: {url}"));
    Ok(())
}

async fn add_files_to_archive(
    source_path: PathBuf,
    target_path: PathBuf,
    permissions: InitialFilePermissions,
    zip_writer: AsyncZipWriter,
) -> Result<(), GolemError> {
    let target_path = Arc::new(target_path);

    let resolve_path_in_archive = |absolute_path: &Path, depth: usize| {
        if depth == 0 {
            Arc::clone(&target_path)
        } else {
            let path_difference = absolute_path.strip_prefix(&source_path).unwrap();
            Arc::new(target_path.join(path_difference))
        }
    };

    let mut add_file_to_archive_tasks = JoinSet::new();

    for entry in WalkDir::new(&source_path).follow_links(true) {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .metadata()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            add_file_to_archive_tasks.spawn(add_file_to_archive(
                File::open(entry.path()).await?,
                resolve_path_in_archive(entry.path(), entry.depth()),
                permissions,
                zip_writer.clone(),
            ));
        }
    }
    while let Some(result) = add_file_to_archive_tasks.join_next().await {
        result??
    }
    Ok(())
}

async fn add_file_to_archive(
    source_file: File,
    target_path: Arc<PathBuf>,
    permissions: InitialFilePermissions,
    zip_writer: AsyncZipWriter,
) -> Result<(), GolemError> {
    let source_file_size = source_file.metadata().await?.len();

    let zip_file_options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(source_file_size >= u32::MAX as u64)
        .unix_permissions(permissions.to_unix_file_permissions());

    let zip_writer = zip_writer.lock().await?;
    zip_writer
        .start_file_from_path(target_path, zip_file_options)
        .await?;

    let mut source_file_stream = ReaderStream::new(source_file);
    while let Some(chunk) = source_file_stream.next().await {
        zip_writer.write_all(chunk?).await?;
    }
    Ok(())
}
