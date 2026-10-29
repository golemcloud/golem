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
use crate::operation_failure::OperationContext;

/// Opens the target according to the expected kind, access mode, symlink policy, and disposition.
///
/// Requires an admitted reconstruction or resident handle and a target from the same generation.
/// Invalid lifecycle or generation state returns `AccessError` immediately. Native, permission,
/// quota, capacity, or invalidation errors are produced by the returned call; success returns an
/// open node whose lifetime delays generation cleanup.
pub(crate) fn open<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    target: PathTarget,
    options: OpenOptions,
) -> Result<FilesystemCall<Opened>, AccessError> {
    let generation = admit(generation_handle)?;
    validate_path_generation(&generation, &target)?;
    if open_requires_mutable_target(options) {
        target.require_mutable()?;
    }
    let opened_access = open_access(options);
    let lease = generation_handle.lease_call(
        &generation.registry,
        if open_changes_filesystem(options) {
            CallEffect::Changes
        } else {
            CallEffect::Read
        },
    )?;
    let invocation = lease.invocation.as_ref().map(InvocationLease::retain);
    Ok(FilesystemCall::new(lease, async move {
        let mut opened =
            execute_coordinated_open(generation, target.sandbox, options, opened_access).await?;
        match &mut opened.node {
            OpenNode::File(file) => file.ownership.invocation = invocation,
            OpenNode::Directory(directory) => directory.ownership.invocation = invocation,
        }
        Ok(opened)
    }))
}

/// Reads up to the requested byte range from an open file.
///
/// Host adapters call this with a file from the same admitted reconstruction or resident
/// generation. Revoked, transitioning, or cross-generation access fails immediately; sandbox read
/// and terminal-generation failures are returned when the deferred call is awaited.
pub(crate) fn read_file<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    file: &File,
    range: ReadRange,
) -> Result<FilesystemCall<ReadResult>, AccessError> {
    let generation = admit_node(generation_handle, file.ownership.generation_id)?;
    if !file.ownership.access.can_read() {
        return Err(AccessError::NotPermitted);
    }
    let lease = generation_handle.lease_call(&generation.registry, CallEffect::Read)?;
    let SandboxHandle::File(file) = file.ownership.sandbox() else {
        unreachable!("file wrapper must contain a sandbox file")
    };
    let file = file.clone();
    Ok(FilesystemCall::new(lease, async move {
        let sandbox = generation.sandbox.read().await;
        let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
        sandbox
            .read(
                &file,
                SandboxReadRange {
                    offset: range.offset,
                    length: range.length,
                },
            )
            .await
            .map_err(|source| classify_query_error(&generation, source))
    }))
}

/// Reads attributes from an open node or from a path target.
///
/// For path targets, `Follow` controls whether the final symlink is dereferenced; open targets
/// address the opened object directly. The target must belong to the admitted reconstruction or
/// resident generation. Admission errors are immediate, while sandbox query errors are deferred.
pub(crate) fn attributes<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    target: Target<'_>,
) -> Result<FilesystemCall<Attributes>, AccessError> {
    let generation = admit(generation_handle)?;
    let target = sandbox_target(&generation, target)?;
    let lease = generation_handle.lease_call(&generation.registry, CallEffect::Read)?;
    Ok(FilesystemCall::new(lease, async move {
        let sandbox = generation.sandbox.read().await;
        let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
        match read_sandbox_attributes(sandbox.as_ref(), target).await {
            Ok(attributes) => Ok(agent_attributes(attributes)),
            Err(source) => Err(classify_query_error(&generation, source)),
        }
    }))
}

/// Tests whether two open nodes refer to the same sandbox filesystem object.
///
/// Host adapters use this for descriptor identity checks. Both nodes must belong to the admitted
/// generation or `WrongGeneration` is returned immediately; sandbox comparison failures are
/// returned by the deferred call.
pub(crate) fn is_same_object<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    left: &OpenNode,
    right: &OpenNode,
) -> Result<FilesystemCall<bool>, AccessError> {
    let left_ownership = node_ownership(left);
    let generation = admit_node(generation_handle, left_ownership.generation_id)?;
    let right_ownership = node_ownership(right);
    if right_ownership.generation_id != left_ownership.generation_id {
        return Err(AccessError::WrongGeneration);
    }
    let lease = generation_handle.lease_call(&generation.registry, CallEffect::Read)?;
    let left = left_ownership.sandbox().clone();
    let right = right_ownership.sandbox().clone();
    Ok(FilesystemCall::new(lease, async move {
        let sandbox = generation.sandbox.read().await;
        let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
        sandbox
            .is_same_open_object(left, right)
            .await
            .map_err(|source| classify_query_error(&generation, source))
    }))
}

/// Lists the immediate entries of an open directory.
///
/// The directory must belong to the admitted reconstruction or resident generation. Lifecycle and
/// generation errors are returned immediately; directory-read failures or terminal invalidation
/// are returned by the deferred call.
pub(crate) fn list_directory<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    directory: &Directory,
) -> Result<FilesystemCall<DirectoryEntries>, AccessError> {
    let generation = admit_node(generation_handle, directory.ownership.generation_id)?;
    if !directory.ownership.access.can_read() {
        return Err(AccessError::NotPermitted);
    }
    let lease = generation_handle.lease_call(&generation.registry, CallEffect::Read)?;
    let SandboxHandle::Directory(directory) = directory.ownership.sandbox() else {
        unreachable!("directory wrapper must contain a sandbox directory")
    };
    let directory = directory.clone();
    Ok(FilesystemCall::new(lease, async move {
        let sandbox = generation.sandbox.read().await;
        let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
        match sandbox.read_directory(&directory).await {
            Ok(entries) => Ok(entries
                .into_iter()
                .map(|entry| DirectoryEntry {
                    name: entry.name,
                    kind: agent_object_kind(entry.kind),
                })
                .collect()),
            Err(source) => Err(classify_query_error(&generation, source)),
        }
    }))
}

/// Reads the stored target of a symlink at a path without following the final link.
///
/// The path must belong to the admitted reconstruction or resident generation. Admission and
/// generation errors are immediate; missing paths, wrong object kinds, and sandbox failures are
/// returned by the deferred call.
pub(crate) fn symlink_target<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    target: PathTarget,
) -> Result<FilesystemCall<SymlinkTarget>, AccessError> {
    let generation = admit(generation_handle)?;
    validate_path_generation(&generation, &target)?;
    let lease = generation_handle.lease_call(&generation.registry, CallEffect::Read)?;
    let path = target.sandbox.clone();
    Ok(FilesystemCall::new(lease, async move {
        let sandbox = generation.sandbox.read().await;
        let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
        match sandbox.read_link(path).await {
            Ok(target) => Ok(SymlinkTarget(target.0)),
            Err(source) => Err(classify_query_error(&generation, source)),
        }
    }))
}

/// Writes bytes to an open file at an offset or at the coordinated append position.
///
/// The file must belong to the admitted reconstruction or resident generation. Admission failures
/// are immediate. The deferred result distinguishes quota and physical-capacity failures, may
/// report a successfully written prefix, and invalidates the generation when the effect is unsafe
/// to determine.
pub(crate) fn write<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    file: &File,
    placement: WritePlacement,
    bytes: Bytes,
) -> Result<FilesystemCall<WriteResult>, AccessError> {
    let generation = admit_node(generation_handle, file.ownership.generation_id)?;
    if !file.ownership.access.can_write() {
        return Err(AccessError::NotPermitted);
    }
    let lease = generation_handle.lease_call(&generation.registry, CallEffect::Changes)?;
    let SandboxHandle::File(file) = file.ownership.sandbox() else {
        unreachable!("file wrapper must contain a sandbox file")
    };
    let file = file.clone();
    Ok(FilesystemCall::new(lease, async move {
        execute_write(generation, file, placement, bytes).await
    }))
}

/// Flushes an open node's data, or its data and metadata, to sandbox storage.
///
/// Host adapters call this for an open node in the admitted reconstruction or resident generation.
/// Admission errors are immediate. Any sandbox flush failure invalidates the generation
/// and is returned as `RuntimeInvalidated` by the deferred call.
pub(crate) fn flush<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    node: &OpenNode,
    level: FlushLevel,
) -> Result<FilesystemCall<()>, AccessError> {
    let ownership = node_ownership(node);
    let generation = admit_node(generation_handle, ownership.generation_id)?;
    let lease = generation_handle.lease_call(&generation.registry, CallEffect::Read)?;
    let node = ownership.sandbox().clone();
    Ok(FilesystemCall::new(lease, async move {
        execute_flush(generation, node, level).await
    }))
}

/// Opens the object at `target` under the coordination that `options` need, and registers the
/// opened node in the generation.
///
/// An open that changes the object or its contents is authorized from a resolution made under the
/// coordination on the entry. That check runs before the open. With a final symlink, an edit of
/// other entries can put a read-only file at the referent between the check and the open, and the
/// coordination on the entry does not stop it. The open pins one object, and the adapter reports
/// whether that object is a regular file without write permission. That fact binds the check to
/// the object that the caller gets: the function closes such an object and refuses the open.
async fn execute_coordinated_open<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: SandboxPath,
    options: OpenOptions,
    opened_access: AccessMode,
) -> Result<Opened, Error> {
    let Some(kind) = open_namespace_coordination(options) else {
        let opened = execute_open(Arc::clone(&generation), target, options).await?;
        return generation.register_opened(opened, opened_access).await;
    };
    let follow = match options {
        OpenOptions::Existing { follow, .. } | OpenOptions::File { follow, .. } => follow,
    };
    let change = open_requires_mutable_target(options).then(|| sandbox_follow(follow));
    loop {
        let Some(CoordinatedTarget {
            mut coordination,
            resolved,
        }) = coordinated_target(&generation, &target, kind, change).await?
        else {
            continue;
        };
        let opened = execute_open(Arc::clone(&generation), resolved.target(), options).await?;
        if change.is_some()
            && let Err(refusal) = refuse_read_only_change(opened.is_read_only_file())
        {
            // A node of the guest open path closes through `execute_close`, which retries the
            // close and decides the effect of a failed close on the generation.
            execute_close(Arc::clone(&generation), opened).await?;
            return Err(refusal);
        }
        if open_returns_directory(options) {
            let directory_key = opened
                .directory_id()
                .expect("sandbox directory open must carry a coordination identity");
            if !coordination.extend(vec![directory_key]) {
                drop(opened);
                continue;
            }
        }
        return generation.register_opened(opened, opened_access).await;
    }
}

pub(super) async fn execute_open<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: SandboxPath,
    options: OpenOptions,
) -> Result<SandboxHandle, Error> {
    let mut options = options;
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox
                .open(target.clone(), sandbox_open_options(options))
                .await
        };
        match result {
            Ok(opened) => return Ok(opened),
            Err(error) => {
                if error.io_kind() == Some(std::io::ErrorKind::PermissionDenied)
                    && !error.is_sandbox_escape()
                    && !error.cleanup_failed()
                    && open_requires_mutable_target(options)
                {
                    let follow = match options {
                        OpenOptions::Existing { follow, .. } | OpenOptions::File { follow, .. } => {
                            follow
                        }
                    };
                    let attributes = {
                        let sandbox = generation.sandbox.read().await;
                        let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
                        sandbox
                            .get_path_attributes(target.clone(), sandbox_follow(follow))
                            .await
                    };
                    if matches!(attributes, Ok(attributes) if attributes.kind == SandboxObjectKind::File && attributes.read_only)
                    {
                        return Err(Error::Access(AccessError::NotPermitted));
                    }
                }
                if open_changes_filesystem(options) {
                    match open_postcondition(&generation, &target, options).await {
                        Ok(true) => {
                            debug_assert_eq!(
                                decide_effect(
                                    FailureCause::Guest,
                                    EffectEvidence::DesiredPostconditionSatisfied,
                                    budget,
                                ),
                                EffectDecision::Succeed
                            );
                            options = existing_open_after_postcondition(options);
                            continue;
                        }
                        Err(postcondition_error)
                            if invalidates_generation(&postcondition_error) =>
                        {
                            generation.invalidate();
                            return Err(Error::RuntimeInvalidated);
                        }
                        Ok(false) | Err(_) => {}
                    }
                }
                let evidence =
                    if !open_changes_filesystem(options) || error_proves_no_effect(&error) {
                        EffectEvidence::NoEffect
                    } else {
                        EffectEvidence::Unknown {
                            known_completed_prefix: 0,
                        }
                    };
                match decide_generation_effect(&generation, &error, evidence, budget).await {
                    EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
                    EffectDecision::ReturnFailure(cause) => {
                        return Err(classified_error(cause, error));
                    }
                    EffectDecision::Invalidate | EffectDecision::RetryAfterProvenNoEffect => {
                        generation.invalidate();
                        return Err(Error::RuntimeInvalidated);
                    }
                    EffectDecision::ReclaimCapacityThenRetry => {
                        return Err(classified_error(FailureCause::PhysicalCapacity, error));
                    }
                    EffectDecision::Succeed | EffectDecision::RetryUnwrittenSuffix => {
                        generation.invalidate();
                        return Err(Error::RuntimeInvalidated);
                    }
                }
            }
        }
    }
}

async fn open_postcondition<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: &SandboxPath,
    options: OpenOptions,
) -> Result<bool, FilesystemStorageError> {
    let sandbox = generation.sandbox.read().await;
    let sandbox = sandbox.as_ref().ok_or_else(|| {
        FilesystemStorageError::from(OperationContext::verification(
            "inspect deleted filesystem open postcondition",
            None,
        ))
    })?;
    let attributes = sandbox
        .get_path_attributes(
            target.clone(),
            sandbox_follow(match options {
                OpenOptions::Existing { follow, .. } | OpenOptions::File { follow, .. } => follow,
            }),
        )
        .await?;
    Ok(match options {
        OpenOptions::File {
            disposition: FileDisposition::CreateIfMissing,
            ..
        } => attributes.kind == SandboxObjectKind::File,
        OpenOptions::File {
            disposition: FileDisposition::TruncateExisting | FileDisposition::CreateOrTruncate,
            ..
        } => attributes.kind == SandboxObjectKind::File && attributes.size == 0,
        OpenOptions::Existing { .. }
        | OpenOptions::File {
            disposition: FileDisposition::CreateExclusive,
            ..
        } => false,
    })
}

fn open_changes_filesystem(options: OpenOptions) -> bool {
    matches!(options, OpenOptions::File { .. })
}

fn existing_open_after_postcondition(options: OpenOptions) -> OpenOptions {
    match options {
        OpenOptions::File { access, follow, .. } => OpenOptions::Existing {
            expected: ObjectKind::File,
            access,
            follow,
        },
        OpenOptions::Existing { .. } => options,
    }
}

async fn execute_write<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    file: SandboxFile,
    placement: WritePlacement,
    bytes: Bytes,
) -> Result<WriteResult, Error> {
    let _append: Option<tokio::sync::OwnedMutexGuard<()>> = match placement {
        WritePlacement::At(_) => None,
        WritePlacement::Append => Some(file.coordinate_append().await),
    };
    let mut completed = 0u64;
    let mut budget = RetryBudget::new(2);
    while completed < bytes.len() as u64 {
        let (sandbox_placement, remaining) = unwritten_write_suffix(placement, &bytes, completed)?;
        let attempt = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox.write(&file, sandbox_placement, remaining).await
        };
        let SandboxWriteAttempt { written, result } = match attempt {
            Ok(attempt) => attempt,
            Err(_) => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        };
        completed = completed
            .checked_add(written)
            .filter(|completed| *completed <= bytes.len() as u64)
            .ok_or_else(|| {
                generation.invalidate();
                Error::RuntimeInvalidated
            })?;
        let error = match result {
            Ok(()) if completed == bytes.len() as u64 => break,
            Ok(()) if written != 0 => continue,
            Ok(()) => FilesystemStorageError::io(
                "write sandbox filesystem file",
                None,
                std::io::ErrorKind::WriteZero.into(),
            ),
            Err(error) => error,
        };
        let evidence = NonZeroU64::new(completed)
            .map_or(EffectEvidence::NoEffect, EffectEvidence::CompletedPrefix);
        match decide_write_effect(&generation, &error, evidence, budget).await {
            EffectDecision::RetryAfterProvenNoEffect | EffectDecision::RetryUnwrittenSuffix
                if budget.consume() =>
            {
                continue;
            }
            EffectDecision::ReturnFailure(_) if completed != 0 => {
                return Ok(WriteResult { written: completed });
            }
            EffectDecision::ReturnFailure(cause) => {
                return Err(classified_error(cause, error));
            }
            EffectDecision::Invalidate
            | EffectDecision::RetryAfterProvenNoEffect
            | EffectDecision::RetryUnwrittenSuffix => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
            EffectDecision::ReclaimCapacityThenRetry => {
                return Err(classified_error(FailureCause::PhysicalCapacity, error));
            }
            EffectDecision::Succeed => return Ok(WriteResult { written: completed }),
        }
    }
    Ok(WriteResult { written: completed })
}

pub(super) fn unwritten_write_suffix(
    placement: WritePlacement,
    bytes: &Bytes,
    completed: u64,
) -> Result<(SandboxWritePlacement, Bytes), Error> {
    let offset = usize::try_from(completed).map_err(|_| Error::RuntimeInvalidated)?;
    if offset > bytes.len() {
        return Err(Error::RuntimeInvalidated);
    }
    let remaining = bytes.slice(offset..);
    let placement = match placement {
        WritePlacement::At(initial) => SandboxWritePlacement::At(
            initial
                .checked_add(completed)
                .ok_or(Error::RuntimeInvalidated)?,
        ),
        WritePlacement::Append => SandboxWritePlacement::Append,
    };
    Ok((placement, remaining))
}

/// Flushes `node` to stable storage, and invalidates the generation on every error.
///
/// After a failed flush, the data that earlier writes gave to the storage can be lost, so the
/// agent cannot safely continue on this generation. So every error of a flush, also a permission
/// error, invalidates the generation, and the call gives `RuntimeInvalidated`.
async fn execute_flush<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    node: SandboxHandle,
    level: FlushLevel,
) -> Result<(), Error> {
    let result = {
        let sandbox = generation.sandbox.read().await;
        let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
        sandbox.flush(&node, sandbox_flush_level(level)).await
    };
    match result {
        Ok(()) => Ok(()),
        Err(_) => {
            generation.invalidate();
            Err(Error::RuntimeInvalidated)
        }
    }
}

fn agent_attributes(attributes: SandboxAttributes) -> Attributes {
    Attributes {
        kind: agent_object_kind(attributes.kind),
        link_count: attributes.link_count,
        size: attributes.size,
        accessed: attributes.accessed,
        modified: attributes.modified,
        read_only: attributes.read_only,
    }
}

pub(super) fn agent_object_kind(kind: SandboxObjectKind) -> ObjectKind {
    match kind {
        SandboxObjectKind::File => ObjectKind::File,
        SandboxObjectKind::Directory => ObjectKind::Directory,
        SandboxObjectKind::Symlink => ObjectKind::Symlink,
    }
}

fn open_returns_directory(options: OpenOptions) -> bool {
    matches!(
        options,
        OpenOptions::Existing {
            expected: ObjectKind::Directory,
            ..
        }
    )
}

pub(super) fn open_namespace_coordination(
    options: OpenOptions,
) -> Option<NamespaceCoordinationKind> {
    match options {
        OpenOptions::Existing {
            expected: ObjectKind::Directory,
            ..
        } => Some(NamespaceCoordinationKind::Observe),
        OpenOptions::Existing { access, .. } if access.can_set_times() => {
            Some(NamespaceCoordinationKind::Observe)
        }
        OpenOptions::File { .. } => Some(NamespaceCoordinationKind::Edit),
        OpenOptions::Existing { .. } => None,
    }
}

fn open_requires_mutable_target(options: OpenOptions) -> bool {
    match options {
        OpenOptions::Existing { access, .. } => access.can_set_times(),
        OpenOptions::File { .. } => true,
    }
}

fn open_access(options: OpenOptions) -> AccessMode {
    match options {
        OpenOptions::Existing { access, .. } | OpenOptions::File { access, .. } => access,
    }
}

fn sandbox_open_options(options: OpenOptions) -> SandboxOpenOptions {
    match options {
        OpenOptions::Existing {
            expected,
            access,
            follow,
        } => SandboxOpenOptions::Existing {
            expected: sandbox_object_kind(expected),
            access: sandbox_access_mode(access),
            follow: sandbox_follow(follow),
        },
        OpenOptions::File {
            access,
            disposition,
            follow,
        } => SandboxOpenOptions::File {
            access: sandbox_access_mode(access),
            disposition: match disposition {
                FileDisposition::CreateIfMissing => SandboxFileDisposition::CreateIfMissing,
                FileDisposition::CreateExclusive => SandboxFileDisposition::CreateExclusive,
                FileDisposition::TruncateExisting => SandboxFileDisposition::TruncateExisting,
                FileDisposition::CreateOrTruncate => SandboxFileDisposition::CreateOrTruncate,
            },
            follow: sandbox_follow(follow),
        },
    }
}

pub(super) fn sandbox_object_kind(kind: ObjectKind) -> SandboxObjectKind {
    match kind {
        ObjectKind::File => SandboxObjectKind::File,
        ObjectKind::Directory => SandboxObjectKind::Directory,
        ObjectKind::Symlink => SandboxObjectKind::Symlink,
    }
}

fn sandbox_access_mode(mode: AccessMode) -> SandboxAccessMode {
    match mode {
        // Metadata-only handles use a native read open; lifecycle data operations enforce the capability.
        AccessMode::None | AccessMode::Read => SandboxAccessMode::Read,
        AccessMode::ReadAndSetTimes => SandboxAccessMode::ReadAndSetTimes,
        AccessMode::Write => SandboxAccessMode::Write,
        AccessMode::ReadWrite => SandboxAccessMode::ReadWrite,
    }
}

fn sandbox_flush_level(level: FlushLevel) -> SandboxFlushLevel {
    match level {
        FlushLevel::Data => SandboxFlushLevel::Data,
        FlushLevel::DataAndMetadata => SandboxFlushLevel::DataAndMetadata,
    }
}
