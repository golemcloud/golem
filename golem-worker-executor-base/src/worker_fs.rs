use anyhow::anyhow;
use faccess::PathExt;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tokio::{fs, task};
use walkdir::WalkDir;
use zip::read::ZipArchive;

use crate::error::GolemError;
use crate::services::component::{ComponentMetadata, ComponentService};
use golem_common::model::{
    ComponentId, DirectoryInfo, FileInfo, FileListing, FileListingPermissions,
    Path as FileListingPath, WorkerId,
};

pub const SYSTEM_ROOT: &str = "system";

#[derive(Debug)]
pub struct PathsToPreopen {
    pub root_path: PathBuf,
    pub system_path: PathBuf,
    pub component_version_path: PathBuf,
    pub component_current_path: PathBuf,
}

// Mapping inside Worker + Host directory structure
//
// `/`  OR  `.` => `data/worker_files/component/{component_uuid}/worker/{worker_name}/root/`
//
// `/system`  =>  `data/worker_files/component/{component_uuid}/worker/{worker_name}/system/`
//
// `/system/component/version/{component_version}` => `data/worker_files/component/{component_uuid}/version`
//
// `/system/component/current` => `data/worker_files/component/{component_uuid}/version/{component_version}`

pub fn get_worker_host_root_path(worker_id: &WorkerId) -> PathBuf {
    let component_id = &worker_id.component_id;
    let component_uuid = component_id.0;
    let worker_name = &worker_id.worker_name;

    let component_root_path =
        PathBuf::from(format!("data/worker_files/component/{component_uuid}"));
    component_root_path.join(format!("worker/{worker_name}"))
}

pub fn get_preopened_root_path(worker_id: &WorkerId) -> PathBuf {
    get_worker_host_root_path(worker_id).join("root")
}

pub async fn prepare_worker_files_and_get_paths(
    worker_id: &WorkerId,
    component_service: Arc<dyn ComponentService + Send + Sync>,
    component_metadata: &ComponentMetadata,
) -> Result<PathsToPreopen, GolemError> {
    let component_id = &worker_id.component_id;
    let component_uuid = component_id.0;
    let component_version = component_metadata.version;
    let worker_name = &worker_id.worker_name;

    // TODO: Sync Workers reading and writing files? Access FS through an RwLock(s)?

    let component_root_path =
        PathBuf::from(format!("data/worker_files/component/{component_uuid}"));
    let component_version_path = component_root_path.join("version");
    let files_path = Arc::new(component_version_path.join(format!("{component_version}/files")));
    let worker_host_root_path = component_root_path.join(format!("worker/{worker_name}"));
    let root_path = worker_host_root_path.join("root");
    let system_path = worker_host_root_path.join("system");
    let component_current_path = component_version_path.join(component_version.to_string());

    if !files_path.is_dir() {
        fs::create_dir_all(files_path.as_ref()).await?;
        extract_initial_fs_archive(
            Arc::clone(&component_service),
            component_id,
            component_version,
            Arc::clone(&files_path),
        )
        .await?;
    }

    if !worker_host_root_path.is_dir() {
        fs::create_dir_all(&root_path).await?;
        fs::create_dir_all(&system_path).await?;
        copy_or_link_files_to_worker_root(&files_path, &root_path).await?;
    }

    let paths_to_preopen = PathsToPreopen {
        root_path,
        system_path,
        component_version_path,
        component_current_path,
    };

    Ok(paths_to_preopen)
}

async fn copy_or_link_files_to_worker_root(
    files_path: &Path,
    worker_root: &Path,
) -> Result<(), GolemError> {
    let absolute_worker_root_path = worker_root.canonicalize()?;

    let dir_entries = WalkDir::new(files_path).min_depth(1).follow_links(true);

    for dir_entry in dir_entries {
        let dir_entry =
            dir_entry.map_err(|error| anyhow!("Failed to read component dir or file: {error}"))?;

        let dir_entry_metadata = dir_entry
            .metadata()
            .map_err(|error| anyhow!("Failed to read component dir or file metadata: {error}"))?;

        if dir_entry_metadata.is_file() {
            let absolute_path = dir_entry.path();

            let path = dir_entry
                .path()
                .strip_prefix(files_path)
                .map_err(|error| anyhow!("Failed to strip dir or file prefix: {error}"))?
                .to_path_buf();

            let new_path = absolute_worker_root_path.join(path);

            // TODO: Parallelize?
            if absolute_path.readable() {
                let new_path_parent = new_path
                    .parent()
                    .ok_or_else(|| anyhow!("Failed to get file parent '{new_path:?}'"))?;
                fs::create_dir_all(new_path_parent).await.map_err(|error| {
                    anyhow!("Failed to create parent directories for '{new_path:?}': {error}")
                })?;
                if absolute_path.writable() {
                    fs::copy(&absolute_path, &new_path).await.map_err(|error| {
                        anyhow!("Failed to copy '{absolute_path:?}' to '{new_path:?}': {error}")
                    })?;
                } else {
                    // TODO: Debug symlinks with Wasmtime engine
                    // create_file_symlink(&absolute_path, &new_path).await.map_err(|error| {
                    //     anyhow!("Failed to symlink '{absolute_path:?}' to '{new_path:?}': {error}")
                    // })?;
                }
            } else {
                Err(anyhow!("File '{absolute_path:?}' is not readable"))?
            }
        }
    }
    Ok(())
}

async fn extract_initial_fs_archive(
    component_service: Arc<dyn ComponentService + Send + Sync>,
    component_id: &ComponentId,
    component_version: u64,
    target_path: Arc<PathBuf>,
) -> Result<(), GolemError> {
    let maybe_archive_path = component_service
        .get_initial_fs_archive(component_id, component_version)
        .await?;

    if let Some(archive_path) = maybe_archive_path {
        task::spawn_blocking(move || {
            let mut archive = ZipArchive::new(File::open(archive_path.as_path())?)?;
            archive.extract(target_path.as_ref())?;
            Ok::<(), anyhow::Error>(())
        })
        .await
        .map_err(|error| anyhow::format_err!("Task to extract files failed: {error}"))??;
    };
    // TODO: Verify file permissions? Only non-Unix platforms?
    Ok(())
}

pub fn file_listings(
    dir_path: &Path,
    recursive: Option<bool>,
) -> Result<Vec<FileListing>, GolemError> {
    let dir_entries = WalkDir::new(dir_path)
        .min_depth(1)
        .follow_links(true)
        .max_depth(if recursive.unwrap_or_default() {
            usize::MAX
        } else {
            1
        })
        .sort_by_file_name();

    let mut listings = Vec::new();

    for dir_entry in dir_entries {
        let dir_entry =
            dir_entry.map_err(|error| anyhow!("Failed to read worker dir or file: {error}"))?;

        let dir_entry_metadata = dir_entry
            .metadata()
            .map_err(|error| anyhow!("Failed to read worker dir or file metadata: {error}"))?;

        let path = dir_entry
            .path()
            .strip_prefix(dir_path)
            .map_err(|error| anyhow!("Failed to strip dir or file prefix: {error}"))?
            .to_path_buf();

        let permissions = file_listing_permissions(dir_entry.path(), &path);

        let last_modified = dir_entry_metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .expect("Invalid last modified time")
            .as_secs();

        let path = path
            .into_os_string()
            .into_string()
            .map(FileListingPath)
            .map_err(|os_string| {
                anyhow!("Path '{os_string:?}' does not contain only valid Unicode data")
            })?;

        let file_listing = if dir_entry_metadata.is_dir() {
            FileListing::Directory(DirectoryInfo {
                path,
                last_modified,
                permissions,
            })
        } else {
            let size = dir_entry_metadata.len();
            FileListing::File(FileInfo {
                path,
                last_modified,
                permissions,
                size,
            })
        };
        listings.push(file_listing);
    }
    Ok(listings)
}

fn file_listing_permissions(
    absolute_path: &Path,
    path_in_worker: &Path,
) -> Option<FileListingPermissions> {
    if !absolute_path.readable() {
        return None;
    }
    if path_in_worker.starts_with(SYSTEM_ROOT) {
        return Some(FileListingPermissions::AlwaysReadOnly);
    }
    if absolute_path.writable() {
        return Some(FileListingPermissions::ReadWrite);
    }
    Some(FileListingPermissions::ReadOnly)
}

// TODO: Try symlinks again later when the engine is more stable or remove functions below

pub async fn create_dir_symlink(dir: &Path, symlink: &Path) -> Result<(), GolemError> {
    create_dir_symlink_internal(dir, symlink)
        .await
        .map_err(|error| {
            GolemError::runtime(format!(
                "Failed to create dir symlink '{dir:?}' to '{symlink:?}': {error}"
            ))
        })
}

#[cfg(unix)]
async fn create_dir_symlink_internal(dir: &Path, symlink: &Path) -> tokio::io::Result<()> {
    tokio::fs::symlink(dir, symlink).await
}

#[cfg(windows)]
async fn create_dir_symlink_internal(dir: &Path, symlink: &Path) -> tokio::io::Result<()> {
    tokio::fs::symlink_dir(dir, symlink).await
}

pub async fn create_file_symlink(file: &Path, symlink: &Path) -> Result<(), GolemError> {
    create_file_symlink_internal(file, symlink)
        .await
        .map_err(|error| {
            GolemError::runtime(format!(
                "Failed to create file symlink '{file:?}' to '{symlink:?}': {error}"
            ))
        })
}

#[cfg(unix)]
async fn create_file_symlink_internal(file: &Path, symlink: &Path) -> tokio::io::Result<()> {
    tokio::fs::symlink(file, symlink).await
}

#[cfg(windows)]
async fn create_file_symlink_internal(file: &Path, symlink: &Path) -> tokio::io::Result<()> {
    tokio::fs::symlink_file(file, symlink).await
}
