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

use super::*;
use rustix::fs::{CloneFlags, StatVfs, StatVfsMountFlags, fclonefileat, fstatvfs};
use std::io::Write;

pub(super) fn bind(
    root: &Path,
    cleanup_retry: RetryConfig,
) -> Result<(FilesystemVolume, directories::DirectoryProvisioning), FilesystemStorageError> {
    bind_with_probe(
        root,
        cleanup_retry,
        |source, parent, name| clone_file(source, parent, name),
        tempfile::TempDir::close,
    )
}

fn bind_with_probe(
    root: &Path,
    cleanup_retry: RetryConfig,
    clone: impl FnOnce(&File, &File, &Path) -> std::io::Result<()>,
    close: impl FnOnce(tempfile::TempDir) -> std::io::Result<()>,
) -> Result<(FilesystemVolume, directories::DirectoryProvisioning), FilesystemStorageError> {
    std::fs::create_dir_all(root)
        .map_err(|error| FilesystemStorageError::io("create APFS development root", root, error))?;
    let directory = File::open(root)
        .map_err(|error| FilesystemStorageError::io("open APFS development root", root, error))?;
    probe_clone(root, clone, close)?;
    tracing::info!(root = %root.display(), "APFS storage is for local development and tests only");
    Ok((
        FilesystemVolume::copy_on_write(Arc::new(directory)),
        directories::DirectoryProvisioning::new(
            Some(Arc::from(root)),
            cleanup_retry,
            NativeNameModeSource::NativeDetection,
        ),
    ))
}

fn probe_clone(
    root: &Path,
    clone: impl FnOnce(&File, &File, &Path) -> std::io::Result<()>,
    close: impl FnOnce(tempfile::TempDir) -> std::io::Result<()>,
) -> Result<(), FilesystemStorageError> {
    let probe = tempfile::Builder::new()
        .prefix(".golem-apfs-clone-probe-")
        .tempdir_in(root)
        .map_err(|error| FilesystemStorageError::io("create APFS clone probe", root, error))?;
    let result = probe_contents(probe.path(), clone);
    let cleanup = close(probe);
    result.map_err(|error| {
        let operation = "probe APFS clone for local development";
        if cleanup.is_err() {
            FilesystemStorageError::cleanup_io(operation, root, error)
        } else {
            FilesystemStorageError::io(operation, root, error)
        }
    })?;
    cleanup
        .map_err(|error| FilesystemStorageError::cleanup_io("remove APFS clone probe", root, error))
}

fn probe_contents(
    path: &Path,
    clone: impl FnOnce(&File, &File, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut source = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path.join("source"))?;
    let contents = [0x5a; 4096];
    source.write_all(&contents)?;
    let parent = File::open(path)?;
    clone(&source, &parent, Path::new("clone"))?;
    if std::fs::read(path.join("clone"))? != contents {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "APFS clone probe changed the file contents",
        ));
    }
    Ok(())
}

pub(super) fn clone_file(
    source: &File,
    parent: impl std::os::fd::AsFd,
    name: &Path,
) -> std::io::Result<()> {
    fclonefileat(
        source,
        parent,
        name,
        CloneFlags::NOFOLLOW | CloneFlags::NOOWNERCOPY,
    )
    .map_err(std::io::Error::from)
}

pub(super) fn observe_space(root: &File) -> std::io::Result<FilesystemSpace> {
    space_from_capacity(&fstatvfs(root).map_err(std::io::Error::from)?)
}

/// Gives the space of an APFS development volume whose `fstatvfs` result is `capacity`. A
/// read-only volume gives a `ReadOnlyFilesystem` error.
fn space_from_capacity(capacity: &StatVfs) -> std::io::Result<FilesystemSpace> {
    if capacity.f_flag.contains(StatVfsMountFlags::RDONLY) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::ReadOnlyFilesystem,
            "APFS development volume is mounted read-only",
        ));
    }
    space_from_values(
        capacity.f_blocks,
        capacity.f_bavail,
        capacity.f_frsize,
        capacity.f_files,
        capacity.f_ffree,
    )
}

#[cfg(test)]
mod tests;
