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

use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::authorization::targets::{
    CanonicalGuestPath, TargetError, agent_owner, filesystem_target,
};
use crate::services::agent_filesystem::{
    self as agent_filesystem, AccessMode, AttributeChanges, Error as AgentFilesystemError,
    File as AgentFile, FileDisposition, FilesystemCall, FilesystemGenerationHandle, FlushLevel,
    Follow, NamespaceEdit, ObjectKind, OpenNode, OpenOptions, Opened, PathTarget, Target,
    TimeChange, TimeChanges, WritePlacement, WriteResult,
};
use crate::workerctx::WorkerCtx;
use bytes::Bytes;
use golem_common::model::card::{FilesystemVerb, PermissionTarget};
use metrohash::MetroHash128;
use wasmtime::component::{Resource, ResourceTableError};

pub(crate) mod p2;
pub(crate) mod p3;

#[derive(Clone)]
pub(crate) struct AgentDescriptor {
    node: Arc<Mutex<OpenNode>>,
    path: Arc<PathBuf>,
}

impl AgentDescriptor {
    /// Wraps an opened agent-filesystem node and its guest path for use as a P2 or P3 descriptor.
    /// Clones share both values so streams and resource-table entries refer to the same open node.
    pub(crate) fn new(node: OpenNode, path: PathBuf) -> Self {
        Self {
            node: Arc::new(Mutex::new(node)),
            path: Arc::new(path),
        }
    }

    /// Runs a P2/P3 descriptor operation while holding the shared open-node lock.
    pub(crate) fn with_node<T>(&self, function: impl FnOnce(&OpenNode) -> T) -> T {
        function(&self.node.lock().unwrap())
    }

    /// Runs a two-descriptor operation with both open nodes locked in a stable order.
    /// Aliases are locked once, avoiding self-deadlock during P2/P3 identity checks.
    pub(crate) fn with_nodes<T>(
        left: &Self,
        right: &Self,
        function: impl FnOnce(&OpenNode, &OpenNode) -> T,
    ) -> T {
        if Arc::ptr_eq(&left.node, &right.node) {
            let node = left.node.lock().unwrap();
            return function(&node, &node);
        }
        if Arc::as_ptr(&left.node).addr() < Arc::as_ptr(&right.node).addr() {
            let left_node = left.node.lock().unwrap();
            let right_node = right.node.lock().unwrap();
            function(&left_node, &right_node)
        } else {
            let right_node = right.node.lock().unwrap();
            let left_node = left.node.lock().unwrap();
            function(&left_node, &right_node)
        }
    }

    /// Returns the guest-visible path recorded when this P2/P3 descriptor was opened.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// Resolves a descriptor-relative WASI path into the canonical absolute guest path used by
/// filesystem permission targets. Resolution cannot escape the descriptor's guest path.
pub(crate) fn agent_descriptor_guest_path(
    descriptor: &AgentDescriptor,
    relative: &str,
) -> Result<CanonicalGuestPath, TargetError> {
    canonical_guest_path_from_descriptor_path(descriptor.path(), relative)
}

fn canonical_guest_path_from_descriptor_path(
    descriptor_path: &Path,
    relative: &str,
) -> Result<CanonicalGuestPath, TargetError> {
    let descriptor_path = descriptor_path
        .to_str()
        .ok_or_else(|| TargetError::InvalidPath(descriptor_path.display().to_string()))?;
    let absolute = match descriptor_path {
        "" | "." => "/".to_string(),
        path if path.starts_with('/') => path.to_string(),
        path => format!("/{path}"),
    };
    CanonicalGuestPath::new(&absolute)?.resolve(relative)
}

pub(crate) fn filesystem_permission_targets<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    requests: &[(FilesystemVerb, CanonicalGuestPath)],
) -> Vec<PermissionTarget> {
    let owner = agent_owner(ctx);
    requests
        .iter()
        .map(|(verb, path)| filesystem_target(owner.clone(), *verb, path))
        .collect()
}

/// Looks up a borrowed P2/P3 descriptor resource and clones its shared agent descriptor.
/// The resource-table entry remains owned by the caller; an invalid handle returns a table error.
pub(crate) fn get_agent_descriptor<T: 'static>(
    ctx: &mut impl AgentFilesystemResources,
    resource: &Resource<T>,
) -> Result<AgentDescriptor, ResourceTableError> {
    ctx.agent_filesystem_table()
        .get(&Resource::<AgentDescriptor>::new_borrow(resource.rep()))
        .cloned()
}

/// Inserts an agent descriptor into the shared WASI resource table and returns an owned P2/P3 handle.
/// The typed handle preserves the inserted representation and transfers table ownership to the guest.
pub(crate) fn push_agent_descriptor<T: 'static>(
    ctx: &mut impl AgentFilesystemResources,
    descriptor: AgentDescriptor,
) -> Result<Resource<T>, ResourceTableError> {
    let resource = ctx.agent_filesystem_table().push(descriptor)?;
    Ok(Resource::new_own(resource.rep()))
}

/// Consumes an owned P2/P3 descriptor handle and removes its agent descriptor from the resource table.
/// Dropping the final shared descriptor starts the agent-filesystem node close; bad ownership traps.
pub(crate) fn delete_agent_descriptor<T: 'static>(
    ctx: &mut impl AgentFilesystemResources,
    resource: Resource<T>,
) -> Result<(), ResourceTableError> {
    ctx.agent_filesystem_table()
        .delete(Resource::<AgentDescriptor>::new_own(resource.rep()))?;
    Ok(())
}

pub(crate) trait AgentFilesystemResources {
    /// Returns the resource table shared by P2 and P3 filesystem descriptors.
    fn agent_filesystem_table(&mut self) -> &mut wasmtime::component::ResourceTable;
}

impl<Ctx: WorkerCtx> AgentFilesystemResources for DurableWorkerCtx<Ctx> {
    fn agent_filesystem_table(&mut self) -> &mut wasmtime::component::ResourceTable {
        self.table()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AgentOpenRequest {
    pub(crate) create: bool,
    pub(crate) directory: bool,
    pub(crate) exclusive: bool,
    pub(crate) truncate: bool,
    pub(crate) follow: bool,
    pub(crate) read: bool,
    pub(crate) write: bool,
    pub(crate) unsupported_sync: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentOpenPolicyError {
    Invalid,
    Unsupported,
    SymlinkLoop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentOpenDecision {
    ObserveAttributes { access: AccessMode, follow: Follow },
    Open(OpenOptions),
}

pub(crate) enum AgentOpenRouteError {
    Invalid,
    Unsupported,
    SymlinkLoop,
    Filesystem(AgentFilesystemError),
}

impl From<AgentOpenPolicyError> for AgentOpenRouteError {
    fn from(error: AgentOpenPolicyError) -> Self {
        match error {
            AgentOpenPolicyError::Invalid => Self::Invalid,
            AgentOpenPolicyError::Unsupported => Self::Unsupported,
            AgentOpenPolicyError::SymlinkLoop => Self::SymlinkLoop,
        }
    }
}

/// Applies the common P2/P3 open policy to WASI flags without touching the filesystem.
/// It rejects unsupported sync flags and invalid directory mutations, or returns the required open step.
pub(crate) fn decide_agent_open(
    request: AgentOpenRequest,
) -> Result<AgentOpenDecision, AgentOpenPolicyError> {
    if request.unsupported_sync {
        return Err(AgentOpenPolicyError::Unsupported);
    }
    if request.directory && (request.create || request.exclusive || request.truncate) {
        return Err(AgentOpenPolicyError::Invalid);
    }

    let access = match (request.read, request.write) {
        (true, true) => AccessMode::ReadWrite,
        (false, true) => AccessMode::Write,
        (true, false) | (false, false) => AccessMode::Read,
    };
    let follow = if request.follow {
        Follow::Yes
    } else {
        Follow::No
    };
    if request.create || request.truncate {
        let disposition = match (request.create, request.exclusive, request.truncate) {
            (true, true, _) => FileDisposition::CreateExclusive,
            (true, false, true) => FileDisposition::CreateOrTruncate,
            (true, false, false) => FileDisposition::CreateIfMissing,
            (false, _, true) => FileDisposition::TruncateExisting,
            (false, _, false) => unreachable!("non-mutating open handled below"),
        };
        Ok(AgentOpenDecision::Open(OpenOptions::File {
            access,
            disposition,
            follow,
        }))
    } else if request.directory {
        Ok(AgentOpenDecision::Open(OpenOptions::Existing {
            expected: ObjectKind::Directory,
            access,
            follow,
        }))
    } else {
        Ok(AgentOpenDecision::ObserveAttributes { access, follow })
    }
}

/// Completes policy for a non-mutating P2/P3 open after agent-filesystem attributes reveal the node kind.
/// Opening an observed symlink without a concrete target kind returns `SymlinkLoop`.
pub(crate) fn decide_agent_existing_open(
    access: AccessMode,
    follow: Follow,
    observed: ObjectKind,
) -> Result<OpenOptions, AgentOpenPolicyError> {
    if observed == ObjectKind::Symlink {
        Err(AgentOpenPolicyError::SymlinkLoop)
    } else {
        Ok(OpenOptions::Existing {
            expected: observed,
            access,
            follow,
        })
    }
}

/// Awaits an admitted agent-filesystem call for a P2/P3 route.
/// Admission failures and asynchronous operation failures are normalized as `AgentFilesystemError`.
pub(crate) async fn run_agent_filesystem_call<T: Send + 'static>(
    call: Result<FilesystemCall<T>, agent_filesystem::AccessError>,
) -> Result<T, AgentFilesystemError> {
    match call {
        Ok(call) => call.await,
        Err(error) => Err(AgentFilesystemError::Access(error)),
    }
}

/// Submits a P2/P3 file write to `agent_filesystem` and returns its deferred call.
/// Access failures are returned immediately; sandbox, quota, and capacity failures arrive when awaited.
pub(crate) fn route_agent_write(
    generation_handle: &FilesystemGenerationHandle,
    file: &AgentFile,
    placement: WritePlacement,
    bytes: Bytes,
) -> Result<FilesystemCall<WriteResult>, AgentFilesystemError> {
    agent_filesystem::write(generation_handle, file, placement, bytes)
        .map_err(AgentFilesystemError::Access)
}

/// Submits P2/P3 attribute changes to `agent_filesystem` for an open node or path target.
/// Access failures are returned before the deferred call; operation failures arrive when awaited.
pub(crate) fn route_agent_set_attributes(
    generation_handle: &FilesystemGenerationHandle,
    target: Target<'_>,
    changes: AttributeChanges,
) -> Result<FilesystemCall<()>, AgentFilesystemError> {
    agent_filesystem::set_attributes(generation_handle, target, changes)
        .map_err(AgentFilesystemError::Access)
}

/// Submits a P2/P3 data or metadata flush request to `agent_filesystem`.
/// The returned call retains the generation admission needed through sandbox completion.
pub(crate) fn route_agent_flush(
    generation_handle: &FilesystemGenerationHandle,
    node: &OpenNode,
    level: FlushLevel,
) -> Result<FilesystemCall<()>, AgentFilesystemError> {
    agent_filesystem::flush(generation_handle, node, level).map_err(AgentFilesystemError::Access)
}

/// Submits a P2/P3 namespace edit to `agent_filesystem` and returns its deferred call.
/// Callers map immediate access failures and awaited storage failures to their preview's error type.
pub(crate) fn route_agent_namespace_edit(
    generation_handle: &FilesystemGenerationHandle,
    edit: NamespaceEdit,
) -> Result<FilesystemCall<()>, AgentFilesystemError> {
    agent_filesystem::edit_namespace(generation_handle, edit).map_err(AgentFilesystemError::Access)
}

/// Restores durable timestamps through `agent_filesystem` after a P2/P3 replayed stat.
/// Missing timestamps are left unchanged; failures follow the normal deferred attribute-call path.
pub(crate) fn route_replay_timestamp_restoration(
    generation_handle: &FilesystemGenerationHandle,
    target: Target<'_>,
    accessed: Option<std::time::SystemTime>,
    modified: Option<std::time::SystemTime>,
) -> Result<FilesystemCall<()>, AgentFilesystemError> {
    agent_filesystem::restore_times(
        generation_handle,
        target,
        replay_time_changes(accessed, modified),
    )
    .map_err(AgentFilesystemError::Access)
}

/// Builds the agent-filesystem attribute change used by P2/P3 `set-size` calls.
/// Resizing leaves access and modification times unchanged.
pub(crate) fn resize_attribute_changes(size: u64) -> AttributeChanges {
    AttributeChanges::File {
        size,
        times: TimeChanges {
            accessed: TimeChange::Keep,
            modified: TimeChange::Keep,
        },
    }
}

/// Maps the P2/P3 `data_only` flag to the agent-filesystem flush level.
pub(crate) fn flush_level(data_only: bool) -> FlushLevel {
    if data_only {
        FlushLevel::Data
    } else {
        FlushLevel::DataAndMetadata
    }
}

/// Converts durable P2/P3 replay timestamps into agent-filesystem time changes.
/// An absent value means keep the sandbox timestamp rather than clear it.
pub(crate) fn replay_time_changes(
    accessed: Option<std::time::SystemTime>,
    modified: Option<std::time::SystemTime>,
) -> TimeChanges {
    TimeChanges {
        accessed: accessed.map_or(TimeChange::Keep, TimeChange::Set),
        modified: modified.map_or(TimeChange::Keep, TimeChange::Set),
    }
}

/// Advances the offset used by P2/P3 streaming writes while preserving append placement.
/// Offset overflow invalidates the runtime operation instead of wrapping.
pub(crate) fn advance_write_placement(
    placement: WritePlacement,
    written: u64,
) -> Result<WritePlacement, AgentFilesystemError> {
    match placement {
        WritePlacement::At(offset) => offset
            .checked_add(written)
            .map(WritePlacement::At)
            .ok_or(AgentFilesystemError::RuntimeInvalidated),
        WritePlacement::Append => Ok(WritePlacement::Append),
    }
}

/// Opens a P2/P3 path through the shared policy and `agent_filesystem` lifecycle.
/// Existing untyped paths are first queried for their kind; policy and filesystem failures remain distinct.
pub(crate) async fn route_agent_open(
    generation_handle: &FilesystemGenerationHandle,
    target: PathTarget,
    request: AgentOpenRequest,
) -> Result<Opened, AgentOpenRouteError> {
    let options = match decide_agent_open(request)? {
        AgentOpenDecision::Open(options) => options,
        AgentOpenDecision::ObserveAttributes {
            access: mode,
            follow,
        } => {
            let attributes = run_agent_filesystem_call(agent_filesystem::attributes(
                generation_handle,
                Target::Path(&target, follow),
            ))
            .await
            .map_err(AgentOpenRouteError::Filesystem)?;
            decide_agent_existing_open(mode, follow, attributes.kind)?
        }
    };

    run_agent_filesystem_call(agent_filesystem::open(generation_handle, target, options))
        .await
        .map_err(AgentOpenRouteError::Filesystem)
}

/// Computes the stable metadata-hash words shared by P2 and P3 from modification time and size.
/// A missing modification time hashes as zero seconds and nanoseconds.
pub(crate) fn calculate_metadata_hash_parts(modified: Option<(u64, u32)>, size: u64) -> (u64, u64) {
    let mut hasher = MetroHash128::new();

    let (seconds, nanoseconds) = modified.unwrap_or((0, 0));
    hasher.write_u64(seconds);
    hasher.write_u32(nanoseconds);
    hasher.write_u64(size);

    hasher.finish128()
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn guest_paths_are_canonicalized_without_host_paths() {
        assert_eq!(
            canonical_guest_path_from_descriptor_path(Path::new(""), "data/./items/../file")
                .unwrap()
                .as_str(),
            "/data/file"
        );
        assert_eq!(
            canonical_guest_path_from_descriptor_path(Path::new("."), "tmp/value")
                .unwrap()
                .as_str(),
            "/tmp/value"
        );
    }

    #[test]
    fn guest_path_resolution_cannot_escape_descriptor() {
        assert!(
            canonical_guest_path_from_descriptor_path(Path::new(""), "../host-secret").is_err()
        );
        assert!(
            canonical_guest_path_from_descriptor_path(Path::new("data"), "../host-secret").is_err()
        );
        assert!(
            canonical_guest_path_from_descriptor_path(Path::new("data"), "../../host-secret")
                .is_err()
        );
        assert!(
            canonical_guest_path_from_descriptor_path(Path::new("data"), "/host-secret").is_err()
        );
    }
}
