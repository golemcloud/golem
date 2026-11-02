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

use super::host_adapter::sandbox_provisioning::SandboxFilesystemName;
use super::host_adapter::tree_copy::TreeExclusions;
use super::name_comparison::{
    NativeNameComparisonMode, NativeNameCoordinationKey, SandboxNameComparisonKey,
};
use super::object_identity::{NativeFileIdentity, SandboxDirectoryId, SandboxObjectId};
use super::open_handles::{
    AppendCoordinatorRegistry, NativeAppendCoordination, NativeDirectoryHandle, NativeFileHandle,
    SandboxDirectory, SandboxFile, SandboxFilePermissions, SandboxHandle,
};
use super::sandbox_paths::{
    ResolvedPathEntry, SandboxPath, SandboxPathBase, SandboxResolvedPathTarget, split_path_target,
};
use super::{
    DeleteError, InstalledLimits, LinkGroup, SandboxAttributes, SandboxDirectoryEntry,
    SandboxFilesystemAdapter, SandboxFilesystemAllocationReader, SandboxFlushLevel, SandboxFollow,
    SandboxOpenOptions, SandboxReadRange, SandboxSymlinkTarget, SandboxTimeChanges,
    SandboxWriteAttempt, SandboxWritePlacement, SeedEntry,
};
use crate::filesystem::errors::OperationContext;
use crate::filesystem::host_directories::HostPath;
use crate::filesystem::sandbox::FilesystemStorageError;
use crate::filesystem::volume::{AgentAccounting, FilesystemAllocation, FilesystemLimits};
use bytes::Bytes;
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[cfg(test)]
pub(in crate::filesystem::sandbox) fn scripted_handle_error(
    operation: &'static str,
) -> FilesystemStorageError {
    FilesystemStorageError::from(OperationContext::verification(operation, None))
}

#[derive(Clone)]
pub(crate) struct ScriptedSandboxFilesystem {
    pub(in crate::filesystem::sandbox) state: Arc<Mutex<ScriptedState>>,
    accounting: AgentAccounting,
}

impl Debug for ScriptedSandboxFilesystem {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScriptedSandboxFilesystem")
            .finish_non_exhaustive()
    }
}

pub(crate) struct ScriptedSandboxFilesystemProvisioning {
    pub(in crate::filesystem::sandbox) state: Arc<Mutex<ScriptedState>>,
    accounting: AgentAccounting,
}

#[derive(Clone)]
pub(crate) struct ScriptedSandboxFilesystemControl {
    pub(in crate::filesystem::sandbox) state: Arc<Mutex<ScriptedState>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ScriptedSandboxPath {
    pub(crate) base: ScriptedSandboxPathBase,
    pub(crate) path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ScriptedSandboxPathBase {
    Root,
    Directory(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ScriptedSandboxPathCall {
    HardLink {
        source: ScriptedSandboxPath,
        destination: ScriptedSandboxPath,
    },
    Rename {
        source: ScriptedSandboxPath,
        destination: ScriptedSandboxPath,
    },
}

pub(crate) struct ScriptedSandboxFilesystemGate {
    pub(in crate::filesystem::sandbox) started: Arc<tokio::sync::Semaphore>,
    pub(in crate::filesystem::sandbox) released: Arc<tokio::sync::Semaphore>,
    pub(in crate::filesystem::sandbox) completed: Arc<tokio::sync::Semaphore>,
}

#[derive(Clone)]
pub(in crate::filesystem::sandbox) struct ScriptedGateState {
    pub(in crate::filesystem::sandbox) started: Arc<tokio::sync::Semaphore>,
    pub(in crate::filesystem::sandbox) released: Arc<tokio::sync::Semaphore>,
    pub(in crate::filesystem::sandbox) completed: Arc<tokio::sync::Semaphore>,
}

#[derive(Default)]
pub(in crate::filesystem::sandbox) struct ScriptedState {
    pub(in crate::filesystem::sandbox) calls: Vec<String>,
    pub(in crate::filesystem::sandbox) sandbox_path_calls: Vec<ScriptedSandboxPathCall>,
    pub(in crate::filesystem::sandbox) append_contents: HashMap<NativeFileIdentity, Vec<u8>>,
    pub(in crate::filesystem::sandbox) gates: HashMap<String, ScriptedGateState>,
    pub(in crate::filesystem::sandbox) creation: Option<Result<(), FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) resolve_path_target:
        VecDeque<Result<ScriptedPathResolution, FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) open:
        VecDeque<Result<SandboxHandle, FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) read: VecDeque<Result<Bytes, FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) read_directory:
        VecDeque<Result<Vec<SandboxDirectoryEntry>, FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) read_link:
        VecDeque<Result<SandboxSymlinkTarget, FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) write:
        VecDeque<Result<SandboxWriteAttempt, FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) get_attributes:
        VecDeque<Result<SandboxAttributes, FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) set_size: VecDeque<Result<(), FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) set_times: VecDeque<Result<(), FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) create_directory:
        VecDeque<Result<(), FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) create_symlink: VecDeque<Result<(), FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) hard_link: VecDeque<Result<(), FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) rename: VecDeque<Result<(), FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) remove_directory:
        VecDeque<Result<(), FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) unlink_file: VecDeque<Result<(), FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) flush: VecDeque<Result<(), FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) close: VecDeque<Result<(), FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) seed: VecDeque<Result<(), FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) observe_allocation:
        VecDeque<Result<FilesystemAllocation, FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) install_limits:
        VecDeque<Result<InstalledLimits, FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) copy_contents:
        VecDeque<Result<Box<[LinkGroup]>, FilesystemStorageError>>,
    pub(in crate::filesystem::sandbox) delete_and_verify:
        VecDeque<Result<(), FilesystemStorageError>>,
}

pub(in crate::filesystem::sandbox) struct ScriptedPathResolution {
    pub(in crate::filesystem::sandbox) parent_identity: u64,
    pub(in crate::filesystem::sandbox) equivalent_name: OsString,
    pub(in crate::filesystem::sandbox) entry: ResolvedPathEntry,
}

impl ScriptedSandboxFilesystemProvisioning {
    pub(crate) fn new() -> (Self, ScriptedSandboxFilesystemControl) {
        let state = Arc::new(Mutex::new(ScriptedState {
            creation: Some(Ok(())),
            ..ScriptedState::default()
        }));
        (
            Self {
                state: Arc::clone(&state),
                accounting: AgentAccounting::Development,
            },
            ScriptedSandboxFilesystemControl { state },
        )
    }

    pub(crate) fn with_accounting(mut self, accounting: AgentAccounting) -> Self {
        self.accounting = accounting;
        self
    }
}

impl ScriptedSandboxFilesystemControl {
    pub(crate) fn calls(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned")
            .calls
            .clone()
    }

    pub(crate) fn sandbox_path_calls(&self) -> Vec<ScriptedSandboxPathCall> {
        self.state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned")
            .sandbox_path_calls
            .clone()
    }

    pub(crate) fn programmed_append_contents(&self, identity: u64) -> Vec<u8> {
        self.state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned")
            .append_contents
            .get(&NativeFileIdentity::Scripted(format!(
                "programmed-file-{identity}"
            )))
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn set_creation(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned")
            .creation = Some(outcome);
    }

    pub(crate) fn block(&self, operation: impl Into<String>) -> ScriptedSandboxFilesystemGate {
        let operation = operation.into();
        let gate = ScriptedGateState {
            started: Arc::new(tokio::sync::Semaphore::new(0)),
            released: Arc::new(tokio::sync::Semaphore::new(0)),
            completed: Arc::new(tokio::sync::Semaphore::new(0)),
        };
        self.state().gates.insert(operation, gate.clone());
        ScriptedSandboxFilesystemGate {
            started: gate.started,
            released: gate.released,
            completed: gate.completed,
        }
    }

    pub(crate) fn push_open(&self, outcome: Result<SandboxHandle, FilesystemStorageError>) {
        self.state().open.push_back(outcome);
    }

    pub(crate) fn push_path_resolution(
        &self,
        parent_identity: u64,
        equivalent_name: impl Into<OsString>,
        final_directory_identity: Option<u64>,
    ) {
        self.state()
            .resolve_path_target
            .push_back(Ok(ScriptedPathResolution {
                parent_identity,
                equivalent_name: equivalent_name.into(),
                entry: final_directory_identity.map_or(ResolvedPathEntry::Missing, |identity| {
                    ResolvedPathEntry::Directory(SandboxDirectoryId(NativeFileIdentity::Scripted(
                        format!("programmed-directory-{identity}"),
                    )))
                }),
            }));
    }

    /// Programs the next namespace resolution of a target that is not a directory. The flags
    /// tell whether the target, and the object that a final symlink names, is a read-only
    /// file.
    pub(crate) fn push_read_only_resolution(
        &self,
        parent_identity: u64,
        equivalent_name: impl Into<OsString>,
        read_only_file: bool,
        followed_read_only_file: bool,
    ) {
        self.state()
            .resolve_path_target
            .push_back(Ok(ScriptedPathResolution {
                parent_identity,
                equivalent_name: equivalent_name.into(),
                entry: if read_only_file {
                    assert!(
                        followed_read_only_file,
                        "a regular file has the same permissions with or without following"
                    );
                    ResolvedPathEntry::File(SandboxFilePermissions::ReadOnly)
                } else {
                    ResolvedPathEntry::Symlink {
                        referent_read_only: Ok(followed_read_only_file),
                    }
                },
            }));
    }

    pub(crate) fn push_path_resolution_error(&self, error: FilesystemStorageError) {
        self.state().resolve_path_target.push_back(Err(error));
    }

    pub(crate) fn push_read(&self, outcome: Result<Bytes, FilesystemStorageError>) {
        self.state().read.push_back(outcome);
    }

    pub(crate) fn push_read_directory(
        &self,
        outcome: Result<Vec<SandboxDirectoryEntry>, FilesystemStorageError>,
    ) {
        self.state().read_directory.push_back(outcome);
    }

    pub(crate) fn push_read_link(
        &self,
        outcome: Result<SandboxSymlinkTarget, FilesystemStorageError>,
    ) {
        self.state().read_link.push_back(outcome);
    }

    pub(crate) fn push_write(&self, outcome: Result<SandboxWriteAttempt, FilesystemStorageError>) {
        self.state().write.push_back(outcome);
    }

    pub(crate) fn push_get_attributes(
        &self,
        outcome: Result<SandboxAttributes, FilesystemStorageError>,
    ) {
        self.state().get_attributes.push_back(outcome);
    }

    pub(crate) fn push_set_size(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().set_size.push_back(outcome);
    }

    pub(crate) fn push_set_times(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().set_times.push_back(outcome);
    }

    pub(crate) fn push_create_directory(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().create_directory.push_back(outcome);
    }

    pub(crate) fn push_create_symlink(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().create_symlink.push_back(outcome);
    }

    pub(crate) fn push_hard_link(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().hard_link.push_back(outcome);
    }

    pub(crate) fn push_rename(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().rename.push_back(outcome);
    }

    pub(crate) fn push_remove_directory(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().remove_directory.push_back(outcome);
    }

    pub(crate) fn push_unlink_file(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().unlink_file.push_back(outcome);
    }

    pub(crate) fn push_flush(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().flush.push_back(outcome);
    }

    pub(crate) fn push_close(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().close.push_back(outcome);
    }

    pub(crate) fn push_seed(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().seed.push_back(outcome);
    }

    pub(crate) fn push_observe_allocation(
        &self,
        outcome: Result<FilesystemAllocation, FilesystemStorageError>,
    ) {
        self.state().observe_allocation.push_back(outcome);
    }

    pub(crate) fn push_install_limits(
        &self,
        outcome: Result<InstalledLimits, FilesystemStorageError>,
    ) {
        self.state().install_limits.push_back(outcome);
    }

    pub(crate) fn push_copy_contents(
        &self,
        outcome: Result<Box<[LinkGroup]>, FilesystemStorageError>,
    ) {
        self.state().copy_contents.push_back(outcome);
    }

    pub(crate) fn push_delete_and_verify(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().delete_and_verify.push_back(outcome);
    }

    fn state(&self) -> std::sync::MutexGuard<'_, ScriptedState> {
        self.state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned")
    }
}

impl ScriptedSandboxFilesystemGate {
    pub(crate) async fn wait_started(&self) {
        self.started
            .acquire()
            .await
            .expect("scripted sandbox filesystem start gate closed")
            .forget();
    }

    pub(crate) fn release(&self) {
        self.released.add_permits(1);
    }

    pub(crate) async fn wait_completed(&self) {
        self.completed
            .acquire()
            .await
            .expect("scripted sandbox filesystem completion gate closed")
            .forget();
    }
}

impl SandboxFilesystemAdapter for ScriptedSandboxFilesystem {
    type Provisioning = ScriptedSandboxFilesystemProvisioning;
    type AllocationReader = ScriptedSandboxFilesystem;

    fn agent_accounting(&self) -> AgentAccounting {
        self.accounting
    }

    async fn create_fresh(
        provisioning: Self::Provisioning,
        name: SandboxFilesystemName,
        limits: Option<FilesystemLimits>,
    ) -> Result<Self, FilesystemStorageError> {
        let mut state = provisioning
            .state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned");
        state.calls.push(format!(
            "create_fresh(name={}, limits={limits:?})",
            name.relative_path().display()
        ));
        take_outcome(&mut state.creation, "create_fresh")?;
        drop(state);
        Ok(Self {
            state: provisioning.state,
            accounting: provisioning.accounting,
        })
    }

    fn open(
        &self,
        target: SandboxPath,
        options: SandboxOpenOptions,
    ) -> impl Future<Output = Result<SandboxHandle, FilesystemStorageError>> + Send {
        self.outcome(
            format!("open(target={target:?}, options={options:?})"),
            |state| &mut state.open,
        )
    }

    fn resolve_path_target(
        &self,
        target: SandboxPath,
    ) -> impl Future<Output = Result<SandboxResolvedPathTarget, FilesystemStorageError>> + Send
    {
        let state = Arc::clone(&self.state);
        async move {
            let call = format!("resolve_path_target(target={target:?})");
            let (gate, programmed) = {
                let mut state = state
                    .lock()
                    .expect("scripted sandbox filesystem lock poisoned");
                state.calls.push(call);
                (
                    state.gates.remove("resolve_path_target"),
                    state.resolve_path_target.pop_front(),
                )
            };
            if let Some(gate) = gate {
                gate.started.add_permits(1);
                gate.released
                    .acquire()
                    .await
                    .expect("scripted sandbox filesystem release gate closed")
                    .forget();
                gate.completed.add_permits(1);
            }
            match programmed {
                Some(Ok(programmed)) => scripted_path_resolution(target, programmed),
                Some(Err(error)) => Err(error),
                None => default_scripted_path_resolution(target),
            }
        }
    }

    fn read(
        &self,
        file: &SandboxFile,
        range: SandboxReadRange,
    ) -> impl Future<Output = Result<Bytes, FilesystemStorageError>> + Send {
        self.outcome(format!("read(file={file:?}, range={range:?})"), |state| {
            &mut state.read
        })
    }

    fn read_directory(
        &self,
        directory: &SandboxDirectory,
    ) -> impl Future<Output = Result<Vec<SandboxDirectoryEntry>, FilesystemStorageError>> + Send
    {
        self.outcome(
            format!("read_directory(directory={directory:?})"),
            |state| &mut state.read_directory,
        )
    }

    fn read_link(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<SandboxSymlinkTarget, FilesystemStorageError>> + Send {
        self.outcome(format!("read_link(path={path:?})"), |state| {
            &mut state.read_link
        })
    }

    fn write(
        &self,
        file: &SandboxFile,
        placement: SandboxWritePlacement,
        bytes: Bytes,
    ) -> impl Future<Output = Result<SandboxWriteAttempt, FilesystemStorageError>> + Send {
        let identity = file.append_coordination.identity.clone();
        let appended = bytes.clone();
        let outcome = self.outcome(
            format!("write(file={file:?}, placement={placement:?}, bytes={bytes:?})"),
            |state| &mut state.write,
        );
        let state = Arc::clone(&self.state);
        async move {
            let attempt = outcome.await?;
            if placement == SandboxWritePlacement::Append
                && let Ok(written) = usize::try_from(attempt.written)
                && written <= appended.len()
            {
                state
                    .lock()
                    .expect("scripted sandbox filesystem lock poisoned")
                    .append_contents
                    .entry(identity)
                    .or_default()
                    .extend_from_slice(&appended[..written]);
            }
            Ok(attempt)
        }
    }

    fn get_handle_attributes(
        &self,
        node: SandboxHandle,
    ) -> impl Future<Output = Result<SandboxAttributes, FilesystemStorageError>> + Send {
        self.outcome(format!("get_handle_attributes(node={node:?})"), |state| {
            &mut state.get_attributes
        })
    }

    fn get_path_attributes(
        &self,
        target: SandboxPath,
        follow: SandboxFollow,
    ) -> impl Future<Output = Result<SandboxAttributes, FilesystemStorageError>> + Send {
        self.outcome(
            format!("get_path_attributes(target={target:?}, follow={follow:?})"),
            |state| &mut state.get_attributes,
        )
    }

    fn is_same_open_object(
        &self,
        left: SandboxHandle,
        right: SandboxHandle,
    ) -> impl Future<Output = Result<bool, FilesystemStorageError>> + Send {
        let same = match (left, right) {
            (SandboxHandle::File(left), SandboxHandle::File(right)) => {
                match (&left.handle, &right.handle) {
                    (NativeFileHandle::Scripted(left), NativeFileHandle::Scripted(right)) => {
                        left == right
                    }
                    _ => false,
                }
            }
            (SandboxHandle::Directory(left), SandboxHandle::Directory(right)) => {
                left.directory_id == right.directory_id
            }
            _ => false,
        };
        async move { Ok(same) }
    }

    fn set_size(
        &self,
        file: &SandboxFile,
        size: u64,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(format!("set_size(file={file:?}, size={size})"), |state| {
            &mut state.set_size
        })
    }

    fn set_handle_times(
        &self,
        node: SandboxHandle,
        times: SandboxTimeChanges,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(
            format!("set_handle_times(node={node:?}, times={times:?})"),
            |state| &mut state.set_times,
        )
    }

    fn set_path_times(
        &self,
        target: SandboxPath,
        follow: SandboxFollow,
        times: SandboxTimeChanges,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(
            format!("set_path_times(target={target:?}, follow={follow:?}, times={times:?})"),
            |state| &mut state.set_times,
        )
    }

    fn create_directory(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(format!("create_directory(path={path:?})"), |state| {
            &mut state.create_directory
        })
    }

    fn create_symlink(
        &self,
        path: SandboxPath,
        target: SandboxSymlinkTarget,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(
            format!("create_symlink(path={path:?}, target={target:?})"),
            |state| &mut state.create_symlink,
        )
    }

    fn hard_link(
        &self,
        source: SandboxPath,
        destination: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned")
            .sandbox_path_calls
            .push(ScriptedSandboxPathCall::HardLink {
                source: scripted_sandbox_path(&source),
                destination: scripted_sandbox_path(&destination),
            });
        self.outcome(
            format!("hard_link(source={source:?}, destination={destination:?})"),
            |state| &mut state.hard_link,
        )
    }

    fn rename(
        &self,
        source: SandboxPath,
        destination: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned")
            .sandbox_path_calls
            .push(ScriptedSandboxPathCall::Rename {
                source: scripted_sandbox_path(&source),
                destination: scripted_sandbox_path(&destination),
            });
        self.outcome(
            format!("rename(source={source:?}, destination={destination:?})"),
            |state| &mut state.rename,
        )
    }

    fn remove_directory(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(format!("remove_directory(path={path:?})"), |state| {
            &mut state.remove_directory
        })
    }

    fn unlink_file(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(format!("unlink_file(path={path:?})"), |state| {
            &mut state.unlink_file
        })
    }

    fn flush(
        &self,
        node: &SandboxHandle,
        level: SandboxFlushLevel,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(format!("flush(node={node:?}, level={level:?})"), |state| {
            &mut state.flush
        })
    }

    fn close(
        &self,
        node: SandboxHandle,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(format!("close(node={node:?})"), |state| &mut state.close)
    }

    fn seed(
        &self,
        entries: Box<[SeedEntry]>,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let entries = entries
            .iter()
            .map(|entry| {
                format!(
                    "{{source={}, target={:?}, access={:?}, placement={:?}}}",
                    entry.source.as_path().display(),
                    entry.target,
                    entry.access,
                    entry.placement
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        self.outcome(format!("seed(entries=[{entries}])"), |state| {
            &mut state.seed
        })
    }

    fn observe_allocation(
        &self,
    ) -> impl Future<Output = Result<FilesystemAllocation, FilesystemStorageError>> + Send {
        self.outcome("observe_allocation()".to_string(), |state| {
            &mut state.observe_allocation
        })
    }

    fn allocation_reader(&self) -> Self::AllocationReader {
        self.clone()
    }

    fn install_limits(
        &self,
        limits: FilesystemLimits,
    ) -> impl Future<Output = Result<InstalledLimits, FilesystemStorageError>> + Send {
        self.outcome(format!("install_limits(limits={limits:?})"), |state| {
            &mut state.install_limits
        })
    }

    fn copy_contents(
        &self,
        source: SandboxPath,
        excluded: Arc<TreeExclusions>,
        target: &HostPath,
    ) -> impl Future<Output = Result<Box<[LinkGroup]>, FilesystemStorageError>> + Send {
        let mut excluded = excluded
            .paths()
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        excluded.sort();
        self.outcome(
            format!(
                "copy_contents(source={source:?}, excluded={excluded:?}, target={})",
                target.as_path().display()
            ),
            |state| &mut state.copy_contents,
        )
    }

    async fn delete_and_verify(self) -> Result<(), DeleteError<Self>> {
        match self
            .outcome("delete_and_verify()".to_string(), |state| {
                &mut state.delete_and_verify
            })
            .await
        {
            Ok(()) => Ok(()),
            Err(source) => Err(DeleteError::new(self, source)),
        }
    }
}

impl SandboxFilesystemAllocationReader for ScriptedSandboxFilesystem {
    fn read_allocation(
        &self,
    ) -> impl Future<Output = Result<FilesystemAllocation, FilesystemStorageError>> + Send {
        self.outcome("observe_allocation()".to_string(), |state| {
            &mut state.observe_allocation
        })
    }
}

impl ScriptedSandboxFilesystem {
    fn outcome<T: Send + 'static>(
        &self,
        call: String,
        outcomes: fn(&mut ScriptedState) -> &mut VecDeque<Result<T, FilesystemStorageError>>,
    ) -> impl Future<Output = Result<T, FilesystemStorageError>> + Send + use<T> {
        scripted_outcome(Arc::clone(&self.state), call, outcomes)
    }
}

pub(in crate::filesystem::sandbox) async fn scripted_outcome<T: Send + 'static>(
    state: Arc<Mutex<ScriptedState>>,
    call: String,
    outcomes: fn(&mut ScriptedState) -> &mut VecDeque<Result<T, FilesystemStorageError>>,
) -> Result<T, FilesystemStorageError> {
    let operation = call
        .split_once('(')
        .map_or_else(|| call.clone(), |(operation, _)| operation.to_string());
    let (gate, outcome) = {
        let mut state = state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned");
        state.calls.push(call);
        let gate = state.gates.remove(&operation);
        let outcome = outcomes(&mut state).pop_front().unwrap_or_else(|| {
            Err(FilesystemStorageError::from(
                OperationContext::verification(
                    "consume programmed scripted sandbox filesystem outcome",
                    None,
                ),
            ))
        });
        (gate, outcome)
    };
    if let Some(gate) = gate {
        gate.started.add_permits(1);
        gate.released
            .acquire()
            .await
            .expect("scripted sandbox filesystem release gate closed")
            .forget();
        gate.completed.add_permits(1);
    }
    outcome
}

pub(in crate::filesystem::sandbox) fn scripted_path_resolution(
    target: SandboxPath,
    programmed: ScriptedPathResolution,
) -> Result<SandboxResolvedPathTarget, FilesystemStorageError> {
    let (_, name) = split_path_target(&target.path).map_err(|error| {
        FilesystemStorageError::io(
            "resolve scripted sandbox filesystem namespace target",
            None,
            error,
        )
    })?;
    let parent_identity = NativeFileIdentity::Scripted(format!(
        "programmed-directory-{}",
        programmed.parent_identity
    ));
    let parent_key = SandboxDirectoryId(parent_identity);
    Ok(SandboxResolvedPathTarget {
        parent: SandboxDirectory::scripted_with_identity(
            programmed.parent_identity,
            format!("programmed-directory-{}", programmed.parent_identity),
        ),
        name: name.into_boxed_os_str(),
        // A programmed canonical label proves equivalence, not byte-exact lookup.
        parent_byte_exact_lookup_proven: false,
        name_comparison_key: SandboxNameComparisonKey {
            parent: parent_key,
            name: NativeNameCoordinationKey {
                name: programmed.equivalent_name,
                mode: NativeNameComparisonMode::Exact,
            },
        },
        entry: programmed.entry,
    })
}

pub(in crate::filesystem::sandbox) fn scripted_sandbox_path(
    path: &SandboxPath,
) -> ScriptedSandboxPath {
    let base = match &path.base {
        SandboxPathBase::Root => ScriptedSandboxPathBase::Root,
        SandboxPathBase::Directory(directory) => {
            ScriptedSandboxPathBase::Directory(match &directory.handle {
                NativeDirectoryHandle::Scripted(id) => *id,
                NativeDirectoryHandle::Host(_) => {
                    panic!("scripted adapter received a production native directory")
                }
            })
        }
    };
    ScriptedSandboxPath {
        base,
        path: path.path.clone(),
    }
}

pub(in crate::filesystem::sandbox) fn default_scripted_path_resolution(
    target: SandboxPath,
) -> Result<SandboxResolvedPathTarget, FilesystemStorageError> {
    let (relative_parent, name) = split_path_target(&target.path).map_err(|error| {
        FilesystemStorageError::io(
            "resolve scripted sandbox filesystem namespace target",
            None,
            error,
        )
    })?;
    let relative_parent = normalize_scripted_parent(&relative_parent);
    let (base_id, base_identity) = match target.base {
        SandboxPathBase::Directory(directory) => match directory.handle {
            NativeDirectoryHandle::Scripted(id) => (id, directory.directory_id.0),
            NativeDirectoryHandle::Host(_) => {
                return Err(scripted_handle_error("resolve scripted namespace target"));
            }
        },
        SandboxPathBase::Root => (0, NativeFileIdentity::Scripted("scripted-root".to_string())),
    };
    let parent_identity = if relative_parent.as_os_str().is_empty() {
        base_identity
    } else {
        NativeFileIdentity::Scripted(format!("{base_identity:?}/{}", relative_parent.display()))
    };
    let parent_handle = if relative_parent.as_os_str().is_empty() {
        base_id
    } else {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        parent_identity.hash(&mut hasher);
        hasher.finish()
    };
    let parent_key = SandboxDirectoryId(parent_identity.clone());
    Ok(SandboxResolvedPathTarget {
        parent: SandboxDirectory::scripted_with_identity(
            parent_handle,
            match parent_identity {
                NativeFileIdentity::Scripted(identity) => identity,
                #[allow(unreachable_patterns)]
                _ => unreachable!("scripted namespace parent identity must be scripted"),
            },
        ),
        parent_byte_exact_lookup_proven: true,
        name_comparison_key: SandboxNameComparisonKey {
            parent: parent_key,
            name: NativeNameCoordinationKey {
                name: name.clone(),
                mode: NativeNameComparisonMode::Exact,
            },
        },
        name: name.into_boxed_os_str(),
        entry: ResolvedPathEntry::Missing,
    })
}

pub(in crate::filesystem::sandbox) fn normalize_scripted_parent(path: &Path) -> PathBuf {
    path.components()
        .fold(PathBuf::new(), |mut normalized, component| {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    normalized.pop();
                }
                std::path::Component::Normal(component) => normalized.push(component),
                std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
                std::path::Component::RootDir => normalized.clear(),
            }
            normalized
        })
}

pub(in crate::filesystem::sandbox) fn take_outcome(
    outcome: &mut Option<Result<(), FilesystemStorageError>>,
    operation: &'static str,
) -> Result<(), FilesystemStorageError> {
    outcome.take().unwrap_or_else(|| {
        Err(FilesystemStorageError::from(
            OperationContext::verification(operation, None),
        ))
    })
}

impl SandboxFile {
    #[cfg(test)]
    pub(crate) fn scripted(id: u64) -> Self {
        Self {
            handle: NativeFileHandle::Scripted(id),
            path: PathBuf::from(format!("<scripted-file-{id}>")),
            append_coordination: NativeAppendCoordination::new(
                NativeFileIdentity::Scripted(format!("scripted-file-{id}")),
                Arc::new(AppendCoordinatorRegistry::default()),
            ),
            permissions: SandboxFilePermissions::Writable,
        }
    }
}

impl SandboxDirectory {
    #[cfg(test)]
    pub(crate) fn scripted(id: u64) -> Self {
        Self::scripted_with_identity(id, format!("directory-{id}"))
    }
}

impl SandboxDirectory {
    #[cfg(test)]
    pub(in crate::filesystem::sandbox) fn scripted_with_identity(
        id: u64,
        identity: String,
    ) -> Self {
        Self {
            handle: NativeDirectoryHandle::Scripted(id),
            path: PathBuf::from(format!("<scripted-directory-{id}>")),
            directory_id: SandboxDirectoryId(NativeFileIdentity::Scripted(identity)),
        }
    }
}

impl SandboxHandle {
    #[cfg(test)]
    pub(crate) fn scripted_file(id: u64) -> Self {
        Self::File(SandboxFile::scripted(id))
    }
}

impl SandboxHandle {
    #[cfg(test)]
    pub(crate) fn scripted_read_only_file(id: u64) -> Self {
        let mut file = SandboxFile::scripted(id);
        file.permissions = SandboxFilePermissions::ReadOnly;
        Self::File(file)
    }
}

impl SandboxHandle {
    #[cfg(test)]
    pub(crate) fn scripted_file_aliases(
        first_id: u64,
        second_id: u64,
        identity: u64,
    ) -> (Self, Self) {
        let coordinators = Arc::new(AppendCoordinatorRegistry::default());
        let identity = NativeFileIdentity::Scripted(format!("programmed-file-{identity}"));
        let file = |id, coordinators| SandboxFile {
            handle: NativeFileHandle::Scripted(id),
            path: PathBuf::from(format!("<scripted-file-{id}>")),
            append_coordination: NativeAppendCoordination::new(identity.clone(), coordinators),
            permissions: SandboxFilePermissions::Writable,
        };
        (
            Self::File(file(first_id, Arc::clone(&coordinators))),
            Self::File(file(second_id, coordinators)),
        )
    }
}

impl SandboxHandle {
    #[cfg(test)]
    pub(crate) fn scripted_directory(id: u64) -> Self {
        Self::Directory(SandboxDirectory::scripted(id))
    }
}

impl SandboxHandle {
    #[cfg(test)]
    pub(crate) fn scripted_directory_with_identity(id: u64, identity: u64) -> Self {
        Self::Directory(SandboxDirectory::scripted_with_identity(
            id,
            format!("programmed-directory-{identity}"),
        ))
    }
}

impl SandboxObjectId {
    #[cfg(test)]
    pub(crate) fn scripted(id: u64) -> Self {
        Self(NativeFileIdentity::Scripted(format!(
            "programmed-object-{id}"
        )))
    }
}

impl SandboxNameComparisonKey {
    #[cfg(test)]
    pub(crate) fn scripted_exact(parent: u64, name: impl Into<OsString>) -> Self {
        Self::scripted(parent, name.into(), NativeNameComparisonMode::Exact)
    }
}

impl SandboxNameComparisonKey {
    #[cfg(test)]
    pub(crate) fn scripted_conservative(parent: u64, name: impl Into<OsString>) -> Self {
        Self::scripted(parent, name.into(), NativeNameComparisonMode::Conservative)
    }
}

impl SandboxNameComparisonKey {
    #[cfg(test)]
    pub(in crate::filesystem::sandbox) fn scripted_windows_insensitive(
        parent: u64,
        name: impl Into<OsString>,
    ) -> Self {
        Self::scripted(
            parent,
            name.into(),
            NativeNameComparisonMode::WindowsInsensitive,
        )
    }
}

impl SandboxNameComparisonKey {
    #[cfg(test)]
    fn scripted(parent: u64, name: OsString, mode: NativeNameComparisonMode) -> Self {
        Self {
            parent: SandboxDirectoryId(NativeFileIdentity::Scripted(format!(
                "comparison-parent-{parent}"
            ))),
            name: NativeNameCoordinationKey { name, mode },
        }
    }
}

#[cfg(test)]
mod tests;
