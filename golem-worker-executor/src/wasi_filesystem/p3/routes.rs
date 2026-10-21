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

fn p3_descriptor_guest_path(
    descriptor: &AgentDescriptor,
    relative: &str,
) -> FilesystemResult<CanonicalGuestPath> {
    agent_descriptor_guest_path(descriptor, relative)
        .map_err(|_| types::ErrorCode::NotPermitted.into())
}

/// Handles a P3 `open-at` request through the shared agent-filesystem open policy and lifecycle.
/// It returns an opened node for resource-table insertion and maps policy or filesystem failures to P3.
pub(crate) async fn route_open(
    generation_handle: &FilesystemGenerationHandle,
    target: PathTarget,
    path_flags: types::PathFlags,
    open_flags: types::OpenFlags,
    descriptor_flags: types::DescriptorFlags,
    interrupt: FilesystemInterruptSignal,
) -> FilesystemResult<agent_filesystem::Opened> {
    route_agent_open(
        generation_handle,
        target,
        p3_agent_open_request(path_flags, open_flags, descriptor_flags),
        interrupt,
    )
    .await
    .map_err(p3_agent_open_error)
}

#[cfg(test)]
pub(crate) async fn route_read_file(
    generation_handle: &FilesystemGenerationHandle,
    file: &AgentFile,
    offset: u64,
    length: usize,
) -> FilesystemResult<Bytes> {
    run_agent_filesystem_call(agent_filesystem::read_file(
        generation_handle,
        file,
        agent_filesystem::ReadRange { offset, length },
    ))
    .await
    .map_err(p3_agent_error)
}

#[cfg(test)]
pub(crate) async fn route_attributes(
    generation_handle: &FilesystemGenerationHandle,
    target: AgentTarget<'_>,
) -> FilesystemResult<types::DescriptorStat> {
    let attributes =
        run_agent_filesystem_call(agent_filesystem::attributes(generation_handle, target))
            .await
            .map_err(p3_agent_error)?;
    p3_agent_stat(attributes)
}

#[cfg(test)]
pub(crate) async fn route_list_directory(
    generation_handle: &FilesystemGenerationHandle,
    directory: &agent_filesystem::Directory,
) -> FilesystemResult<Vec<types::DirectoryEntry>> {
    run_agent_filesystem_call(agent_filesystem::list_directory(
        generation_handle,
        directory,
    ))
    .await
    .map_err(p3_agent_error)?
    .into_iter()
    .map(|entry| {
        Ok(types::DirectoryEntry {
            type_: p3_agent_descriptor_type(entry.kind),
            name: entry
                .name
                .into_string()
                .map_err(|_| FilesystemError::from(types::ErrorCode::IllegalByteSequence))?,
        })
    })
    .collect()
}

/// Resolves a P3 `readlink-at` target through `agent_filesystem`.
/// A target that is not valid Unicode returns `IllegalByteSequence`.
pub(crate) async fn route_symlink_target(
    generation_handle: &FilesystemGenerationHandle,
    target: PathTarget,
) -> FilesystemResult<String> {
    let target =
        run_agent_filesystem_call(agent_filesystem::symlink_target(generation_handle, target))
            .await
            .map_err(p3_agent_error)?;
    target
        .0
        .into_os_string()
        .into_string()
        .map_err(|_| types::ErrorCode::IllegalByteSequence.into())
}

async fn route_namespace_edit(
    generation_handle: &FilesystemGenerationHandle,
    edit: NamespaceEdit,
) -> FilesystemResult<()> {
    let call = route_agent_namespace_edit(generation_handle, edit).map_err(p3_agent_error)?;
    call.await.map_err(p3_agent_error)
}

/// Creates a directory for P3 by submitting an agent-filesystem namespace insertion.
pub(crate) async fn route_create_directory(
    generation_handle: &FilesystemGenerationHandle,
    destination: PathTarget,
) -> FilesystemResult<()> {
    route_namespace_edit(
        generation_handle,
        NamespaceEdit::Insert {
            destination,
            object: NewObject::Directory,
        },
    )
    .await
}

/// Creates a symlink for P3 by submitting its destination and raw target to `agent_filesystem`.
pub(crate) async fn route_create_symlink(
    generation_handle: &FilesystemGenerationHandle,
    destination: PathTarget,
    target: impl Into<PathBuf>,
) -> FilesystemResult<()> {
    route_namespace_edit(
        generation_handle,
        NamespaceEdit::Insert {
            destination,
            object: NewObject::Symlink(SymlinkTarget(target.into())),
        },
    )
    .await
}

/// Creates a P3 hard link through the agent-filesystem namespace editor.
/// Following the source symlink is invalid, and cross-generation links report `CrossDevice`.
pub(crate) async fn route_hard_link(
    generation_handle: &FilesystemGenerationHandle,
    source: PathTarget,
    source_flags: types::PathFlags,
    destination: PathTarget,
) -> FilesystemResult<()> {
    if source_flags.contains(types::PathFlags::SYMLINK_FOLLOW) {
        return Err(types::ErrorCode::Invalid.into());
    }
    let call = match route_agent_namespace_edit(
        generation_handle,
        NamespaceEdit::Link {
            source,
            destination,
        },
    ) {
        Err(AgentFilesystemError::Access(error)) => return Err(p3_link_access_error(error)),
        result => result.map_err(p3_agent_error)?,
    };
    call.await.map_err(p3_agent_error)
}

/// Renames a P3 path through an agent-filesystem namespace move.
pub(crate) async fn route_rename(
    generation_handle: &FilesystemGenerationHandle,
    source: PathTarget,
    destination: PathTarget,
) -> FilesystemResult<()> {
    route_namespace_edit(
        generation_handle,
        NamespaceEdit::Move {
            source,
            destination,
        },
    )
    .await
}

/// Removes a P3 directory through `agent_filesystem`, requiring the target to be a directory.
pub(crate) async fn route_remove_directory(
    generation_handle: &FilesystemGenerationHandle,
    target: PathTarget,
) -> FilesystemResult<()> {
    route_namespace_edit(
        generation_handle,
        NamespaceEdit::Remove {
            target,
            expected: ObjectKind::Directory,
        },
    )
    .await
}

/// Unlinks a P3 path through `agent_filesystem` while enforcing the caller's expected object kind.
pub(crate) async fn route_unlink(
    generation_handle: &FilesystemGenerationHandle,
    target: PathTarget,
    expected: ObjectKind,
) -> FilesystemResult<()> {
    route_namespace_edit(
        generation_handle,
        NamespaceEdit::Remove { target, expected },
    )
    .await
}

#[cfg(test)]
pub(crate) fn route_append_via_stream_chunk(
    generation_handle: &FilesystemGenerationHandle,
    file: &AgentFile,
    bytes: Bytes,
) -> FilesystemResult<impl Future<Output = P3WriteFutureResult> + Send + 'static + use<>> {
    route_stream_chunk(generation_handle, file, WritePlacement::Append, bytes)
}

pub(super) fn route_stream_chunk(
    generation_handle: &FilesystemGenerationHandle,
    file: &AgentFile,
    placement: WritePlacement,
    bytes: Bytes,
) -> FilesystemResult<impl Future<Output = P3WriteFutureResult> + Send + 'static + use<>> {
    let call =
        route_agent_write(generation_handle, file, placement, bytes).map_err(p3_agent_error)?;
    Ok(async move { p3_agent_write_result(call.await, placement) })
}

/// Starts a P3 file resize through the agent-filesystem attribute route.
/// The returned future owns the admitted call and reports its final P3 result.
pub(crate) fn route_set_size(
    generation_handle: &FilesystemGenerationHandle,
    node: &OpenNode,
    size: types::Filesize,
) -> FilesystemResult<impl Future<Output = FilesystemResult<()>> + Send + 'static + use<>> {
    let call = route_agent_set_attributes(
        generation_handle,
        AgentTarget::Open(node),
        resize_attribute_changes(size),
    )
    .map_err(p3_agent_error)?;
    Ok(async move { call.await.map_err(p3_agent_error) })
}

/// Starts a P3 timestamp update for an open descriptor or path through `agent_filesystem`.
/// Invalid timestamps fail before submission; the returned future reports operation failures.
pub(crate) fn route_set_times(
    generation_handle: &FilesystemGenerationHandle,
    target: AgentTarget<'_>,
    accessed: types::NewTimestamp,
    modified: types::NewTimestamp,
) -> FilesystemResult<impl Future<Output = FilesystemResult<()>> + Send + 'static + use<>> {
    let changes = p3_time_changes(accessed, modified)?;
    let call =
        route_agent_set_attributes(generation_handle, target, AttributeChanges::Times(changes))
            .map_err(p3_agent_error)?;
    Ok(async move { call.await.map_err(p3_agent_error) })
}

/// Starts a P3 descriptor flush through `agent_filesystem`.
/// `data_only` selects a data-only flush rather than a data-and-metadata flush.
pub(crate) fn route_flush(
    generation_handle: &FilesystemGenerationHandle,
    node: &OpenNode,
    data_only: bool,
) -> FilesystemResult<impl Future<Output = FilesystemResult<()>> + Send + 'static + use<>> {
    let call = route_agent_flush(generation_handle, node, flush_level(data_only))
        .map_err(p3_agent_error)?;
    Ok(async move { call.await.map_err(p3_agent_error) })
}

#[cfg(test)]
pub(crate) fn route_replay_times(
    generation_handle: &FilesystemGenerationHandle,
    target: AgentTarget<'_>,
    accessed: Option<std::time::SystemTime>,
    modified: Option<std::time::SystemTime>,
) -> FilesystemResult<impl Future<Output = FilesystemResult<()>> + Send + 'static + use<>> {
    let call = crate::wasi_filesystem::route_replay_timestamp_restoration(
        generation_handle,
        target,
        accessed,
        modified,
    )
    .map_err(p3_agent_error)?;
    Ok(async move { call.await.map_err(p3_agent_error) })
}

pub(super) fn descriptor_guest_path_from_accessor<Ctx: WorkerCtx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
    relative: &str,
) -> FilesystemResult<CanonicalGuestPath>
where
    U: 'static,
{
    let descriptor = store
        .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, fd))
        .map_err(FilesystemError::trap)?;
    p3_descriptor_guest_path(&descriptor, relative)
}

pub(super) fn descriptor_path_from_accessor<Ctx: WorkerCtx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<PathBuf>
where
    U: 'static,
{
    store.with(|mut access| {
        Ok(agent_descriptor_from_access::<Ctx, U>(&mut access, fd)?
            .path()
            .to_path_buf())
    })
}

pub(super) fn descriptor_path_at_from_accessor<Ctx: WorkerCtx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
    path: &str,
) -> wasmtime::Result<PathBuf>
where
    U: 'static,
{
    store.with(|mut access| {
        Ok(agent_descriptor_from_access::<Ctx, U>(&mut access, fd)?
            .path()
            .join(path))
    })
}

pub(super) fn agent_descriptor_from_access<Ctx: WorkerCtx, U>(
    store: &mut Access<'_, U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<AgentDescriptor>
where
    U: 'static,
{
    Ok(get_agent_descriptor(
        durable_worker_ctx::<Ctx, U>(store.data_mut()),
        fd,
    )?)
}

pub(super) fn agent_path_target(
    descriptor: &AgentDescriptor,
    path: impl Into<PathBuf>,
) -> FilesystemResult<PathTarget> {
    descriptor.with_node(|node| match node {
        OpenNode::Directory(directory) => Ok(PathTarget::at(directory, path)),
        OpenNode::File(_) => Err(types::ErrorCode::NotDirectory.into()),
    })
}

pub(super) fn agent_descriptor_flags(
    descriptor: &AgentDescriptor,
) -> FilesystemResult<types::DescriptorFlags> {
    let (kind, mode) = descriptor.with_node(|node| (node.kind(), node.access()));
    let mut flags = types::DescriptorFlags::empty();
    if matches!(
        mode,
        agent_filesystem::AccessMode::Read
            | agent_filesystem::AccessMode::ReadAndSetTimes
            | agent_filesystem::AccessMode::ReadWrite
    ) {
        flags |= types::DescriptorFlags::READ;
    }
    if matches!(
        mode,
        agent_filesystem::AccessMode::Write | agent_filesystem::AccessMode::ReadWrite
    ) {
        flags |= if kind == ObjectKind::Directory {
            types::DescriptorFlags::MUTATE_DIRECTORY
        } else {
            types::DescriptorFlags::WRITE
        };
    }
    Ok(flags)
}

pub(super) fn push_agent_descriptor_from_accessor<Ctx: WorkerCtx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    descriptor: AgentDescriptor,
) -> FilesystemResult<Resource<Descriptor>>
where
    U: 'static,
{
    accessor
        .with(|mut access| {
            push_agent_descriptor(durable_worker_ctx::<Ctx, U>(access.data_mut()), descriptor)
        })
        .map_err(FilesystemError::trap)
}
