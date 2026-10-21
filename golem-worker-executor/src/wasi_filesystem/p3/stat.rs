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

/// Runs the check of `agent_filesystem::is_immutable_initial_file` for a stat of `fd`, or of the
/// path below `fd` that `at` gives: `true` when the stat reads Golem's read-only initial file at
/// `guest_path`, so its volatile timestamps can be removed without an oplog entry.
///
/// A path below a descriptor that is not a directory gives `false`, so the durable stat gives
/// and records its error. A failure of the check traps, as a failure of the stat of the fast path
/// does, because a guest error that the oplog does not record could differ in a replay.
pub(super) async fn p3_is_immutable_initial_file<Ctx: WorkerCtx, U: 'static>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
    guest_path: &CanonicalGuestPath,
    at: Option<(types::PathFlags, String)>,
) -> FilesystemResult<bool> {
    let root_relative_path =
        std::path::Path::new(guest_path.as_str().strip_prefix('/').unwrap_or_default());
    let generation_handle = store.with(|mut access| {
        durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
    });
    let descriptor = store
        .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, fd))
        .map_err(FilesystemError::trap)?;
    let check = match at {
        None => descriptor.with_node(|node| {
            agent_filesystem::is_immutable_initial_file(
                &generation_handle,
                root_relative_path,
                AgentTarget::Open(node),
            )
        }),
        Some((path_flags, path)) => {
            let Ok(target) = agent_path_target(&descriptor, path) else {
                return Ok(false);
            };
            let follow = if path_flags.contains(types::PathFlags::SYMLINK_FOLLOW) {
                agent_filesystem::Follow::Yes
            } else {
                agent_filesystem::Follow::No
            };
            agent_filesystem::is_immutable_initial_file(
                &generation_handle,
                root_relative_path,
                AgentTarget::Path(&target, follow),
            )
        }
    };
    match check.map_err(|error| p3_agent_error(AgentFilesystemError::Access(error)))? {
        None => Ok(false),
        Some(call) => call.await.map_err(|error| {
            FilesystemError::trap(wasmtime::Error::msg(format!(
                "immutable initial-file check failed: {error}"
            )))
        }),
    }
}

pub(super) async fn p3_stable_initial_file_stat(
    call: impl Future<
        Output = wasmtime::Result<Result<types::DescriptorStat, SerializableP3FileSystemError>>,
    >,
) -> FilesystemResult<types::DescriptorStat> {
    let mut stat = call
        .await
        .map_err(FilesystemError::trap)?
        .map_err(|error| {
            FilesystemError::trap(wasmtime::Error::msg(format!(
                "immutable initial-file stat failed: {error:?}"
            )))
        })?;
    stat.data_access_timestamp = None;
    stat.data_modification_timestamp = None;
    Ok(stat)
}

fn serialize_stat_error(error: &FilesystemError) -> SerializableP3FileSystemError {
    SerializableP3FileSystemError::from_result(
        error
            .downcast_ref()
            .cloned()
            .ok_or_else(|| error.to_string()),
    )
}

fn deserialize_stat_error(error: SerializableP3FileSystemError) -> FilesystemError {
    match error {
        SerializableP3FileSystemError::ErrorCode(error_code) => {
            types::ErrorCode::from(error_code).into()
        }
        SerializableP3FileSystemError::Generic(error) => {
            FilesystemError::trap(wasmtime::Error::msg(error))
        }
    }
}

pub(super) fn serialize_stat_result(
    stat: &Result<types::DescriptorStat, SerializableP3FileSystemError>,
) -> Result<SerializableFileTimes, SerializableP3FileSystemError> {
    stat.clone().map(|stat| SerializableFileTimes {
        data_access_timestamp: stat.data_access_timestamp.map(Into::into),
        data_modification_timestamp: stat.data_modification_timestamp.map(Into::into),
    })
}

pub(super) async fn run_local_stat<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: Resource<Descriptor>,
) -> wasmtime::Result<Result<types::DescriptorStat, SerializableP3FileSystemError>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let generation_handle = store.with(|mut access| {
        durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
    });
    let descriptor =
        match store.with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd)) {
            Ok(descriptor) => descriptor,
            Err(error) => {
                return Ok(Err(SerializableP3FileSystemError::Generic(
                    error.to_string(),
                )));
            }
        };
    let call = match descriptor.with_node(|node| {
        agent_filesystem::attributes(&generation_handle, AgentTarget::Open(node))
            .map_err(|error| p3_agent_error(AgentFilesystemError::Access(error)))
    }) {
        Ok(call) => call,
        Err(error) => return Ok(Err(serialize_stat_error(&error))),
    };
    let result = observe_filesystem_operation(filesystem_interrupt::<Ctx, U>(store), call)
        .await
        .map_err(wasmtime::Error::from)?;
    Ok(result
        .map_err(p3_agent_error)
        .and_then(p3_agent_stat)
        .map_err(|error| serialize_stat_error(&error)))
}

pub(super) async fn run_local_stat_at<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: Resource<Descriptor>,
    path_flags: types::PathFlags,
    path: String,
) -> wasmtime::Result<Result<types::DescriptorStat, SerializableP3FileSystemError>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let generation_handle = store.with(|mut access| {
        durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
    });
    let descriptor =
        match store.with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd)) {
            Ok(descriptor) => descriptor,
            Err(error) => {
                return Ok(Err(SerializableP3FileSystemError::Generic(
                    error.to_string(),
                )));
            }
        };
    let target = match agent_path_target(&descriptor, path) {
        Ok(target) => target,
        Err(error) => return Ok(Err(serialize_stat_error(&error))),
    };
    let follow = if path_flags.contains(types::PathFlags::SYMLINK_FOLLOW) {
        agent_filesystem::Follow::Yes
    } else {
        agent_filesystem::Follow::No
    };
    let call = match agent_filesystem::attributes(
        &generation_handle,
        AgentTarget::Path(&target, follow),
    ) {
        Ok(call) => call,
        Err(error) => {
            return Ok(Err(serialize_stat_error(&p3_agent_error(
                AgentFilesystemError::Access(error),
            ))));
        }
    };
    let result = observe_filesystem_operation(filesystem_interrupt::<Ctx, U>(store), call)
        .await
        .map_err(wasmtime::Error::from)?;
    Ok(result
        .map_err(p3_agent_error)
        .and_then(p3_agent_stat)
        .map_err(|error| serialize_stat_error(&error)))
}

pub(super) async fn apply_stat_response(
    stat: Result<types::DescriptorStat, SerializableP3FileSystemError>,
    response: HostResponseP3FileSystemStat,
) -> FilesystemResult<types::DescriptorStat> {
    match response.result {
        Ok(times) => {
            let mut stat = stat.unwrap();
            stat.data_access_timestamp = times.data_access_timestamp.map(Into::into);
            stat.data_modification_timestamp = times.data_modification_timestamp.map(Into::into);
            Ok(stat)
        }
        Err(error) => Err(deserialize_stat_error(error)),
    }
}
