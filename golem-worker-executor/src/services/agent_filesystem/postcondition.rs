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

use std::sync::Arc;
use std::time::SystemTime;

#[cfg(target_os = "linux")]
use cap_std::fs::MetadataExt as _;
use wasmtime_wasi::filesystem::{Descriptor, Dir};
use wasmtime_wasi::runtime::spawn_blocking;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MutationPostcondition {
    Satisfied,
    NoEffect,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ObjectIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PathObjectType {
    Directory,
    RegularFile,
    SymbolicLink,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PathState {
    pub(crate) identity: Option<ObjectIdentity>,
    pub(crate) type_: PathObjectType,
    pub(crate) size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymlinkState {
    pub(crate) object: Option<PathState>,
    pub(crate) target: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TimesState {
    pub(crate) identity: Option<ObjectIdentity>,
    pub(crate) accessed: Option<SystemTime>,
    pub(crate) modified: Option<SystemTime>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestedTime {
    NoChange,
    Now,
    Timestamp { seconds: i128, nanoseconds: u32 },
}

#[cfg(target_os = "linux")]
fn object_identity(metadata: &cap_std::fs::Metadata) -> Option<ObjectIdentity> {
    Some(ObjectIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(target_os = "linux"))]
fn object_identity(_metadata: &cap_std::fs::Metadata) -> Option<ObjectIdentity> {
    None
}

pub(crate) fn same_object(left: PathState, right: PathState) -> bool {
    left.identity
        .zip(right.identity)
        .is_some_and(|(left, right)| left == right)
}

pub(crate) fn same_optional_object(left: Option<PathState>, right: Option<PathState>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => same_object(left, right),
        _ => false,
    }
}

fn path_state_from_metadata(metadata: cap_std::fs::Metadata) -> PathState {
    let file_type = metadata.file_type();
    let type_ = if file_type.is_dir() {
        PathObjectType::Directory
    } else if file_type.is_file() {
        PathObjectType::RegularFile
    } else if file_type.is_symlink() {
        PathObjectType::SymbolicLink
    } else {
        PathObjectType::Other
    };
    PathState {
        identity: object_identity(&metadata),
        type_,
        size: metadata.len(),
    }
}

pub(crate) async fn descriptor_state(descriptor: &Descriptor) -> Result<PathState, std::io::Error> {
    match descriptor {
        Descriptor::File(file) => {
            let file = Arc::clone(&file.file);
            spawn_blocking(move || file.metadata().map(path_state_from_metadata)).await
        }
        Descriptor::Dir(dir) => {
            let dir = Arc::clone(&dir.dir);
            spawn_blocking(move || dir.dir_metadata().map(path_state_from_metadata)).await
        }
    }
}

pub(crate) async fn path_state(
    directory: &Dir,
    path: &str,
) -> Result<Option<PathState>, std::io::Error> {
    path_state_with_follow(directory, path, false).await
}

pub(crate) async fn path_state_with_follow(
    directory: &Dir,
    path: &str,
    follow_symlink: bool,
) -> Result<Option<PathState>, std::io::Error> {
    let directory = Arc::clone(&directory.dir);
    let path = path.to_string();
    spawn_blocking(move || {
        match if follow_symlink {
            directory.metadata(path)
        } else {
            directory.symlink_metadata(path)
        } {
            Ok(metadata) => Ok(Some(path_state_from_metadata(metadata))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    })
    .await
}

pub(crate) async fn read_link(
    directory: &Dir,
    path: &str,
) -> Result<Option<String>, std::io::Error> {
    let directory = Arc::clone(&directory.dir);
    let path = path.to_string();
    spawn_blocking(move || match directory.read_link(path) {
        Ok(path) => path
            .into_os_string()
            .into_string()
            .map(Some)
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    })
    .await
}

pub(crate) async fn symlink_state(
    directory: &Dir,
    path: &str,
) -> Result<SymlinkState, std::io::Error> {
    let object = path_state(directory, path).await?;
    let target = if object.is_some_and(|state| state.type_ == PathObjectType::SymbolicLink) {
        read_link(directory, path)
            .await?
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "symlink disappeared while probing mutation postcondition",
                )
            })?
            .into()
    } else {
        None
    };
    Ok(SymlinkState { object, target })
}

pub(crate) fn state_postcondition(
    current: Result<Option<PathState>, std::io::Error>,
    desired: impl FnOnce(Option<PathState>) -> bool,
    unchanged: impl FnOnce(Option<PathState>) -> bool,
) -> MutationPostcondition {
    match current {
        Ok(current) if desired(current) => MutationPostcondition::Satisfied,
        Ok(current) if unchanged(current) => MutationPostcondition::NoEffect,
        Ok(_) | Err(_) => MutationPostcondition::Unknown,
    }
}

pub(crate) fn create_directory_postcondition(
    before: Option<PathState>,
    current: Result<Option<PathState>, std::io::Error>,
) -> MutationPostcondition {
    state_postcondition(
        current,
        |state| {
            before.is_none() && state.is_some_and(|state| state.type_ == PathObjectType::Directory)
        },
        |state| same_optional_object(before, state),
    )
}

pub(crate) fn remove_postcondition(
    before: Option<PathState>,
    current: Result<Option<PathState>, std::io::Error>,
) -> MutationPostcondition {
    state_postcondition(
        current,
        |state| before.is_some() && state.is_none(),
        |state| same_optional_object(before, state),
    )
}

pub(crate) fn link_postcondition(
    source_before: Option<PathState>,
    destination_before: Option<PathState>,
    source_after: Result<Option<PathState>, std::io::Error>,
    destination_after: Result<Option<PathState>, std::io::Error>,
) -> MutationPostcondition {
    match (source_before, source_after, destination_after) {
        (Some(source), Ok(Some(current_source)), Ok(Some(destination)))
            if same_object(destination, source)
                && same_object(current_source, source)
                && match destination_before {
                    None => true,
                    Some(before) => before
                        .identity
                        .zip(source.identity)
                        .is_some_and(|(before, source)| before != source),
                } =>
        {
            MutationPostcondition::Satisfied
        }
        (Some(source), Ok(Some(current_source)), Ok(None))
            if destination_before.is_none() && same_object(current_source, source) =>
        {
            MutationPostcondition::NoEffect
        }
        (_, Ok(current_source), Ok(current_destination))
            if same_optional_object(source_before, current_source)
                && same_optional_object(destination_before, current_destination) =>
        {
            MutationPostcondition::NoEffect
        }
        _ => MutationPostcondition::Unknown,
    }
}

pub(crate) fn rename_postcondition(
    source_before: Option<PathState>,
    destination_before: Option<PathState>,
    source_after: Result<Option<PathState>, std::io::Error>,
    destination_after: Result<Option<PathState>, std::io::Error>,
) -> MutationPostcondition {
    match (source_before, source_after, destination_after) {
        (Some(source), Ok(None), Ok(Some(destination))) if same_object(destination, source) => {
            MutationPostcondition::Satisfied
        }
        (Some(source), Ok(Some(current_source)), Ok(destination))
            if same_object(current_source, source)
                && same_optional_object(destination_before, destination) =>
        {
            MutationPostcondition::NoEffect
        }
        (_, Ok(current_source), Ok(current_destination))
            if same_optional_object(source_before, current_source)
                && same_optional_object(destination_before, current_destination) =>
        {
            MutationPostcondition::NoEffect
        }
        _ => MutationPostcondition::Unknown,
    }
}

pub(crate) fn symlink_postcondition(
    before: &SymlinkState,
    current: Result<SymlinkState, std::io::Error>,
    target: &str,
) -> MutationPostcondition {
    match current {
        Ok(current)
            if before.target.as_deref() != Some(target)
                && current.target.as_deref() == Some(target) =>
        {
            MutationPostcondition::Satisfied
        }
        Ok(current)
            if same_optional_object(before.object, current.object)
                && before.target == current.target =>
        {
            MutationPostcondition::NoEffect
        }
        Ok(_) | Err(_) => MutationPostcondition::Unknown,
    }
}

pub(crate) fn open_postcondition(
    before: Option<PathState>,
    current: Result<Option<PathState>, std::io::Error>,
    requested_type: PathObjectType,
    truncate: bool,
    exclusive: bool,
) -> MutationPostcondition {
    match current {
        Ok(Some(current))
            if current.type_ == requested_type
                && (!truncate || current.size == 0)
                && (!exclusive || before.is_none()) =>
        {
            MutationPostcondition::Satisfied
        }
        Ok(current)
            if match (before, current) {
                (None, None) => true,
                (Some(before), Some(current)) => {
                    same_object(before, current) && before.size == current.size
                }
                _ => false,
            } =>
        {
            MutationPostcondition::NoEffect
        }
        Ok(_) | Err(_) => MutationPostcondition::Unknown,
    }
}

pub(crate) fn resize_postcondition(
    before: PathState,
    current: Result<PathState, std::io::Error>,
    size: u64,
) -> MutationPostcondition {
    match current {
        Ok(current) if current.size == size => MutationPostcondition::Satisfied,
        Ok(current) if current.size == before.size => MutationPostcondition::NoEffect,
        Ok(_) | Err(_) => MutationPostcondition::Unknown,
    }
}

fn times_state(metadata: cap_std::fs::Metadata) -> TimesState {
    TimesState {
        identity: object_identity(&metadata),
        accessed: metadata.accessed().ok().map(|time| time.into_std()),
        modified: metadata.modified().ok().map(|time| time.into_std()),
    }
}

pub(crate) async fn descriptor_times(
    descriptor: &Descriptor,
) -> Result<TimesState, std::io::Error> {
    match descriptor {
        Descriptor::File(file) => {
            let file = Arc::clone(&file.file);
            spawn_blocking(move || file.metadata().map(times_state)).await
        }
        Descriptor::Dir(dir) => {
            let dir = Arc::clone(&dir.dir);
            spawn_blocking(move || dir.dir_metadata().map(times_state)).await
        }
    }
}

pub(crate) async fn path_times(
    directory: &Dir,
    path: &str,
    follow_symlink: bool,
) -> Result<TimesState, std::io::Error> {
    let directory = Arc::clone(&directory.dir);
    let path = path.to_string();
    spawn_blocking(move || {
        if follow_symlink {
            directory.metadata(path).map(times_state)
        } else {
            directory.symlink_metadata(path).map(times_state)
        }
    })
    .await
}

fn requested_time_matches(requested: RequestedTime, actual: Option<SystemTime>) -> bool {
    match requested {
        RequestedTime::NoChange => true,
        RequestedTime::Now => false,
        RequestedTime::Timestamp {
            seconds,
            nanoseconds,
        } => actual.is_some_and(|actual| {
            actual
                .duration_since(SystemTime::UNIX_EPOCH)
                .is_ok_and(|actual| {
                    i128::from(actual.as_secs()) == seconds && actual.subsec_nanos() == nanoseconds
                })
        }),
    }
}

pub(crate) fn times_postcondition(
    current: Result<TimesState, std::io::Error>,
    before: TimesState,
    accessed: RequestedTime,
    modified: RequestedTime,
    identity_required: bool,
) -> MutationPostcondition {
    match current {
        Ok(current)
            if requested_time_matches(accessed, current.accessed)
                && requested_time_matches(modified, current.modified) =>
        {
            MutationPostcondition::Satisfied
        }
        Ok(current)
            if current.accessed == before.accessed
                && current.modified == before.modified
                && (!identity_required
                    || current
                        .identity
                        .zip(before.identity)
                        .is_some_and(|(current, before)| current == before)) =>
        {
            MutationPostcondition::NoEffect
        }
        Ok(_) | Err(_) => MutationPostcondition::Unknown,
    }
}
