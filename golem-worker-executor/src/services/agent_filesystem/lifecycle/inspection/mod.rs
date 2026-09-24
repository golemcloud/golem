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
use crate::sandbox_filesystem::SandboxInspectionFailure;
use golem_common::model::filesystem::{
    FileByteSelection, FileReadError, FileReadHead, FileReadMetadata, validate_file_read_path,
};

#[cfg(test)]
mod tests;

pub(crate) enum FileInspection {
    Opened {
        file: File,
        metadata: FileReadMetadata,
    },
    Rejected(FileReadHead),
}

/// Opens a canonical absolute path without following any intermediate or final symlink.
///
/// The caller must hold the resident Store and exclusive owner lane from before this call until
/// the returned file has finished streaming. This is observation, not a durable guest invocation.
/// Metadata is captured from the opened descriptor; path metadata only rejects unsafe opens.
/// Directories never select an index file. No returned error contains a host path.
pub(crate) async fn open_file_for_inspection<Adapter: SandboxFilesystemAdapter>(
    handle: &FilesystemGenerationHandle<Adapter>,
    path: &str,
    selection: FileByteSelection,
) -> Result<FileInspection, FileReadError> {
    validate_file_read_path(path)?;
    selection.validate()?;
    if !matches!(handle.phase, GenerationHandlePhase::Resident) {
        return Err(FileReadError::Lifecycle);
    }
    let components: Vec<&str> = path[1..]
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    // The empty path is the filesystem root, never a regular file.
    if components.is_empty() {
        let generation = admit(handle).map_err(|_| FileReadError::Lifecycle)?;
        let _lease = generation
            .registry
            .lease_call()
            .map_err(|_| FileReadError::Lifecycle)?;
        return Ok(FileInspection::Rejected(FileReadHead::NotRegular));
    }
    let mut directory = None;
    for (index, component) in components.iter().enumerate() {
        let last = index == components.len() - 1;
        let expected = if last {
            SandboxObjectKind::File
        } else {
            SandboxObjectKind::Directory
        };
        let path = match &directory {
            Some(directory) => PathTarget::at(directory, *component),
            None => {
                PathTarget::at_root(handle, *component).map_err(|_| FileReadError::Lifecycle)?
            }
        };
        let opened = inspection_open(handle, path, expected)
            .map_err(|_| FileReadError::Lifecycle)?
            .await;
        let opened = match opened {
            Ok(opened) => opened,
            Err(error) => return classify_inspection_error(error),
        };
        if last {
            let attrs = match attributes(handle, Target::Open(&opened.node))
                .map_err(|_| FileReadError::Lifecycle)?
                .await
            {
                Ok(attrs) => attrs,
                Err(Error::Sandbox(_)) => return Err(FileReadError::Storage),
                Err(_) => return Err(FileReadError::Lifecycle),
            };
            let OpenNode::File(file) = opened.node else {
                return Ok(FileInspection::Rejected(FileReadHead::NotRegular));
            };
            return Ok(FileInspection::Opened {
                file,
                metadata: FileReadMetadata {
                    total_size: attrs.size,
                    selection: selection.resolve(attrs.size)?,
                    modified_at: attrs.modified.map(Into::into),
                },
            });
        }
        let OpenNode::Directory(next) = opened.node else {
            return Ok(FileInspection::Rejected(FileReadHead::NotRegular));
        };
        directory = Some(next);
    }
    unreachable!("nonempty component walk returns at its final component")
}

fn inspection_open<Adapter: SandboxFilesystemAdapter>(
    handle: &FilesystemGenerationHandle<Adapter>,
    path: PathTarget,
    expected: SandboxObjectKind,
) -> Result<FilesystemCall<Opened>, AccessError> {
    let generation = admit(handle)?;
    validate_path_generation(&generation, &path)?;
    let lease = generation.registry.lease_call()?;
    Ok(FilesystemCall::new(lease, async move {
        let opened = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox
                .open(path.sandbox, SandboxOpenOptions::Inspection { expected })
                .await
                .map_err(|source| {
                    // Read permission denial is a target outcome, not evidence of corruption.
                    if source.io_kind() == Some(std::io::ErrorKind::PermissionDenied) {
                        Error::Sandbox(source)
                    } else {
                        classify_query_error(&generation, source)
                    }
                })?
        };
        generation.register_opened(opened, AccessMode::Read).await
    }))
}

fn classify_inspection_error(error: Error) -> Result<FileInspection, FileReadError> {
    let Error::Sandbox(source) = error else {
        return Err(FileReadError::Lifecycle);
    };
    if let Some(failure) = source
        .io_error()
        .and_then(std::io::Error::get_ref)
        .and_then(|error| error.downcast_ref::<SandboxInspectionFailure>())
    {
        return Ok(FileInspection::Rejected(match failure {
            SandboxInspectionFailure::Symlink => FileReadHead::Symlink,
            SandboxInspectionFailure::NotRegular => FileReadHead::NotRegular,
        }));
    }
    match source.io_kind() {
        Some(std::io::ErrorKind::NotFound) => Ok(FileInspection::Rejected(FileReadHead::Absent)),
        Some(std::io::ErrorKind::PermissionDenied) => {
            Ok(FileInspection::Rejected(FileReadHead::PermissionDenied))
        }
        Some(std::io::ErrorKind::NotADirectory | std::io::ErrorKind::IsADirectory) => {
            Ok(FileInspection::Rejected(FileReadHead::NotRegular))
        }
        _ => Err(FileReadError::Storage),
    }
}
