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

fn p3_agent_storage_error(error: FilesystemStorageError) -> FilesystemError {
    if error.is_sandbox_escape() {
        return types::ErrorCode::Access.into();
    }
    match error.io_error() {
        Some(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            types::ErrorCode::CrossDevice.into()
        }
        Some(error) => types::ErrorCode::from(error).into(),
        None => FilesystemError::trap(wasmtime::Error::msg(error.to_string())),
    }
}

pub(super) fn p3_agent_error(error: AgentFilesystemError) -> FilesystemError {
    match error {
        AgentFilesystemError::Access(agent_filesystem::AccessError::NotPermitted) => {
            types::ErrorCode::NotPermitted.into()
        }
        AgentFilesystemError::Access(agent_filesystem::AccessError::ReadOnly) => {
            types::ErrorCode::ReadOnly.into()
        }
        AgentFilesystemError::Sandbox(error) => p3_agent_storage_error(error),
        AgentFilesystemError::AgentQuota(_) => types::ErrorCode::Quota.into(),
        AgentFilesystemError::PhysicalCapacity(_) => types::ErrorCode::InsufficientSpace.into(),
        // An install of initial files is not a call of the guest, so a conflict and an
        // unavailable or refused initial-file source cannot reach a guest descriptor. It traps with the other internal lifecycle failures.
        error @ (AgentFilesystemError::Access(_)
        | AgentFilesystemError::Baseline(_)
        | AgentFilesystemError::InitialFileConflict(_)
        | AgentFilesystemError::InitialFileUnavailable(_)
        | AgentFilesystemError::InitialFileRefused { .. }
        | AgentFilesystemError::RuntimeInvalidated) => {
            FilesystemError::trap(wasmtime::Error::msg(error.to_string()))
        }
    }
}

pub(super) fn p3_agent_open_error(error: AgentOpenRouteError) -> FilesystemError {
    match error {
        AgentOpenRouteError::Invalid => types::ErrorCode::Invalid.into(),
        AgentOpenRouteError::Unsupported => types::ErrorCode::Unsupported.into(),
        AgentOpenRouteError::SymlinkLoop => types::ErrorCode::Loop.into(),
        AgentOpenRouteError::Filesystem(error) => p3_agent_error(error),
        AgentOpenRouteError::Interrupted(kind) => FilesystemError::trap(kind),
    }
}

pub(super) fn p3_link_access_error(error: agent_filesystem::AccessError) -> FilesystemError {
    match error {
        agent_filesystem::AccessError::WrongGeneration => types::ErrorCode::CrossDevice.into(),
        error => p3_agent_error(AgentFilesystemError::Access(error)),
    }
}

pub(super) fn p3_agent_datetime(
    time: std::time::SystemTime,
) -> FilesystemResult<wasmtime_wasi::p3::bindings::clocks::system_clock::Instant> {
    let (seconds, nanoseconds) = match time.duration_since(std::time::SystemTime::UNIX_EPOCH) {
        Ok(duration) => (
            i64::try_from(duration.as_secs()).map_err(|_| types::ErrorCode::Overflow)?,
            duration.subsec_nanos(),
        ),
        Err(error) => {
            let duration = error.duration();
            let nanoseconds = duration.subsec_nanos();
            let seconds = -i128::from(duration.as_secs()) - i128::from(nanoseconds != 0);
            (
                i64::try_from(seconds).map_err(|_| types::ErrorCode::Overflow)?,
                if nanoseconds == 0 {
                    0
                } else {
                    1_000_000_000 - nanoseconds
                },
            )
        }
    };
    Ok(wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
        seconds,
        nanoseconds,
    })
}

pub(super) fn p3_agent_descriptor_type(kind: ObjectKind) -> types::DescriptorType {
    match kind {
        ObjectKind::File => types::DescriptorType::RegularFile,
        ObjectKind::Directory => types::DescriptorType::Directory,
        ObjectKind::Symlink => types::DescriptorType::SymbolicLink,
    }
}

pub(super) fn p3_agent_stat(
    attributes: AgentAttributes,
) -> FilesystemResult<types::DescriptorStat> {
    Ok(types::DescriptorStat {
        type_: p3_agent_descriptor_type(attributes.kind),
        link_count: attributes.link_count,
        size: attributes.size,
        data_access_timestamp: attributes.accessed.map(p3_agent_datetime).transpose()?,
        data_modification_timestamp: attributes.modified.map(p3_agent_datetime).transpose()?,
        status_change_timestamp: None,
    })
}

pub(super) fn p3_agent_open_request(
    path_flags: types::PathFlags,
    open_flags: types::OpenFlags,
    descriptor_flags: types::DescriptorFlags,
) -> AgentOpenRequest {
    AgentOpenRequest {
        create: open_flags.contains(types::OpenFlags::CREATE),
        directory: open_flags.contains(types::OpenFlags::DIRECTORY),
        exclusive: open_flags.contains(types::OpenFlags::EXCLUSIVE),
        truncate: open_flags.contains(types::OpenFlags::TRUNCATE),
        follow: path_flags.contains(types::PathFlags::SYMLINK_FOLLOW),
        read: descriptor_flags.contains(types::DescriptorFlags::READ),
        write: descriptor_flags.contains(types::DescriptorFlags::WRITE),
        mutate_directory: descriptor_flags.contains(types::DescriptorFlags::MUTATE_DIRECTORY),
        unsupported_sync: descriptor_flags.intersects(
            types::DescriptorFlags::FILE_INTEGRITY_SYNC
                | types::DescriptorFlags::DATA_INTEGRITY_SYNC
                | types::DescriptorFlags::REQUESTED_WRITE_SYNC,
        ),
    }
}

pub(super) fn p3_agent_write_result(
    result: Result<WriteResult, AgentFilesystemError>,
    placement: WritePlacement,
) -> P3WriteFutureResult {
    let result = match result {
        Ok(result) => result,
        Err(error) => return p3_agent_error(error).downcast().map(Err),
    };
    let written = usize::try_from(result.written)
        .map_err(|_| wasmtime::Error::msg("filesystem write progress overflowed"))?;
    let next = advance_write_placement(placement, result.written)
        .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
    Ok(Ok((written, next)))
}

pub(super) fn p3_native_time(
    requested: types::NewTimestamp,
) -> Result<Option<std::time::SystemTime>, FilesystemError> {
    match requested {
        types::NewTimestamp::NoChange => Ok(None),
        types::NewTimestamp::Now => Ok(Some(std::time::SystemTime::now())),
        types::NewTimestamp::Timestamp(timestamp) => p3_system_time(timestamp)
            .map(Some)
            .ok_or_else(|| types::ErrorCode::Overflow.into()),
    }
}

fn p3_system_time(
    timestamp: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
) -> Option<std::time::SystemTime> {
    if timestamp.nanoseconds >= 1_000_000_000 {
        return None;
    }
    if timestamp.seconds >= 0 {
        std::time::SystemTime::UNIX_EPOCH.checked_add(Duration::new(
            timestamp.seconds as u64,
            timestamp.nanoseconds,
        ))
    } else if timestamp.nanoseconds == 0 {
        std::time::SystemTime::UNIX_EPOCH
            .checked_sub(Duration::from_secs(timestamp.seconds.unsigned_abs()))
    } else {
        std::time::SystemTime::UNIX_EPOCH.checked_sub(Duration::new(
            timestamp.seconds.unsigned_abs() - 1,
            1_000_000_000 - timestamp.nanoseconds,
        ))
    }
}

fn p3_validate_time(requested: types::NewTimestamp) -> Result<(), FilesystemError> {
    match requested {
        types::NewTimestamp::Timestamp(timestamp) => p3_system_time(timestamp)
            .map(|_| ())
            .ok_or_else(|| types::ErrorCode::Overflow.into()),
        types::NewTimestamp::NoChange | types::NewTimestamp::Now => Ok(()),
    }
}

pub(super) fn p3_time_changes(
    accessed: types::NewTimestamp,
    modified: types::NewTimestamp,
) -> Result<TimeChanges, FilesystemError> {
    p3_validate_time(accessed)?;
    p3_validate_time(modified)?;
    Ok(TimeChanges {
        accessed: p3_time_change(accessed)?,
        modified: p3_time_change(modified)?,
    })
}

fn p3_time_change(requested: types::NewTimestamp) -> Result<TimeChange, FilesystemError> {
    match requested {
        types::NewTimestamp::NoChange => Ok(TimeChange::Keep),
        types::NewTimestamp::Now => Ok(TimeChange::Now),
        types::NewTimestamp::Timestamp(_) => p3_native_time(requested)?
            .map_or_else(|| Ok(TimeChange::Keep), |time| Ok(TimeChange::Set(time))),
    }
}

#[cfg(test)]
pub(super) fn p3_visible_descriptor_flags(
    mut flags: types::DescriptorFlags,
    read_only: bool,
) -> types::DescriptorFlags {
    if read_only {
        flags &= !types::DescriptorFlags::WRITE;
    }
    flags
}

/// Computes the metadata hash from a durable stat result, using the same hash
/// inputs and function as the WASI P2 implementation so P2 and P3 report
/// identical hashes for the same file state. P3 datetimes use signed seconds
/// while P2 uses unsigned; pre-epoch timestamps fail with `overflow` just like
/// the P2 stat conversion does.
pub(super) fn metadata_hash_from_stat(
    stat: &types::DescriptorStat,
) -> FilesystemResult<types::MetadataHashValue> {
    let modified = stat
        .data_modification_timestamp
        .map(|timestamp| {
            let seconds =
                u64::try_from(timestamp.seconds).map_err(|_| types::ErrorCode::Overflow)?;
            Ok::<_, types::ErrorCode>((seconds, timestamp.nanoseconds))
        })
        .transpose()?;

    let (lower, upper) = calculate_metadata_hash_parts(modified, stat.size);
    Ok(types::MetadataHashValue { lower, upper })
}
