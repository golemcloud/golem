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

use std::path::Path;

#[derive(Debug)]
pub(crate) enum RemoveFailure {
    Io(std::io::Error),
    StillPresent,
}

/// Removes a path without following a symlink, then verifies absence.
pub(crate) fn remove_and_verify(path: &Path) -> Result<(), RemoveFailure> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.is_symlink() => {
            std::fs::remove_dir_all(path)
        }
        Ok(_) => std::fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(RemoveFailure::Io(error)),
    }
    .map_err(RemoveFailure::Io)?;
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RemoveFailure::Io(error)),
        Ok(_) => Err(RemoveFailure::StillPresent),
    }
}

#[cfg(test)]
mod tests;
