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

use super::initial_files::{
    Declarations, InstalledFile, PathLookup, PathReader, PathState, declarations_of, install,
    merged_declarations, observe, seed_with_retry, validate_compatible,
};
use super::*;
use crate::sandbox_filesystem::HostPath;
use futures::{StreamExt as _, TryStreamExt as _};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::ffi::OsStr;
use std::time::Duration;

/// The name of the directory that holds the copied tree in a capture directory.
const TREE_DIRECTORY: &str = "tree";
/// The name of the file that holds the record in a capture directory.
const RECORD_FILE: &str = "record.json";

/// Fills an empty host directory with the contents of a capture directory: `tree/` and the record.
pub(crate) trait RestoreTree: Send {
    /// Fills the empty directory `into`. A failure can leave a part of the contents in `into`.
    fn restore(self, into: &Path) -> impl Future<Output = Result<(), RestoreError>> + Send;
}

impl RestoreTree for Infallible {
    async fn restore(self, _into: &Path) -> Result<(), RestoreError> {
        match self {}
    }
}

/// Why a restore did not fill its directory.
#[derive(Debug)]
pub(crate) struct RestoreError {
    /// Whether a new attempt can succeed without a change.
    #[allow(dead_code)]
    pub(crate) retryable: bool,
    /// The cause of the failure.
    pub(crate) source: anyhow::Error,
}

impl Display for RestoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "failed to restore the agent filesystem baseline: {:#}",
            self.source
        )
    }
}

impl std::error::Error for RestoreError {}

/// A copy of the files of an agent filesystem, with its record, in one host directory.
///
/// The directory holds `tree/` and `record.json`. The capture does not depend on the generation
/// that made it. The directory goes away when the capture is discarded or dropped.
#[allow(dead_code)]
pub(crate) struct FilesystemCapture {
    directory: HostDirectory,
}

#[allow(dead_code)]
impl FilesystemCapture {
    /// The host directory that holds `tree/` and `record.json`.
    pub(crate) fn directory(&self) -> &Path {
        self.directory.path().as_path()
    }

    /// Removes the capture directory with all that is in it.
    pub(crate) async fn discard(self) -> Result<(), FilesystemStorageError> {
        self.directory.discard().await
    }
}

/// Why a capture did not copy a filesystem.
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum CaptureError {
    /// The generation is sealed or invalidated.
    Invalidated,
    /// A filesystem call was still open when the wait ended.
    Busy,
    /// The copy or its record failed.
    Sandbox(FilesystemStorageError),
}

impl Display for CaptureError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalidated => formatter.write_str("agent filesystem access is revoked"),
            Self::Busy => formatter.write_str("agent filesystem has an open call"),
            Self::Sandbox(error) => Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for CaptureError {}

/// The record of a capture, next to `tree/` in the capture directory.
#[derive(serde::Serialize, serde::Deserialize)]
struct CaptureRecord {
    /// The initial-file declarations of the component revision, in path order.
    initial_files: Box<[InitialAgentFile]>,
    /// The entity-provisioned file declarations, in path order.
    provisioned_files: Box<[InitialAgentFile]>,
    /// The read-only files that were still the files that the lifecycle installed at their
    /// paths, in path order.
    read_only_files: Box<[RecordedReadOnlyFile]>,
    /// The files with more than one name, as the copy found them.
    link_groups: Box<[LinkGroup]>,
}

/// A read-only file that was still the file that the lifecycle installed at its path.
#[derive(serde::Serialize, serde::Deserialize)]
struct RecordedReadOnlyFile {
    path: Box<Path>,
    content_hash: AgentFileContentHash,
    /// Whether the tree leaves out the bytes of the file, because the file had a single name.
    left_out: bool,
}

impl CaptureRecord {
    /// Reads the record from the capture directory `directory`.
    async fn read(directory: &HostPath) -> Result<Self, Error> {
        let path = directory
            .child(OsStr::new(RECORD_FILE))
            .map_err(Error::Sandbox)?;
        let bytes = tokio::fs::read(path.as_path()).await.map_err(|error| {
            record_error(anyhow::Error::new(error).context("read the filesystem capture record"))
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            record_error(anyhow::Error::new(error).context("decode the filesystem capture record"))
        })
    }
}

fn record_error(source: anyhow::Error) -> Error {
    Error::Baseline(Box::new(RestoreError {
        retryable: false,
        source,
    }))
}

/// Copies a resident filesystem into a new host directory, and leaves the filesystem resident.
///
/// Use this only at a boundary. The capture stops new filesystem calls (`begin_transition`),
/// waits for the calls that are open (`wait_for_calls`), copies the tree, and opens the filesystem
/// again (`finish_transition`). The capture directory holds `tree/` and `record.json`. The tree is
/// the whole filesystem minus each read-only initial or entity-provisioned file that has a single
/// name and is still the file that the lifecycle installed at its declared path. The tree holds a
/// file with more than one name once. The record gives the read-only files that are still the
/// files that the lifecycle installed at their paths, with their content hashes and whether the
/// tree leaves out their bytes. It also gives the hard-link groups and the declarations.
///
/// The wait for open calls ends at `wait`. A call that is still open then gives `Busy`, and the
/// filesystem opens again at once. A guest that keeps such a call can never be captured, but the
/// capture never blocks it either.
///
/// A call can stay open at a boundary, when no invocation runs. `wasi:io/streams`
/// `output-stream.write` does not block by contract. The guest can call it and return from the
/// invocation without `flush` and without a poll of the stream. On the host,
/// `AgentFileOutputStream::write` (`wasi_filesystem/p2/types.rs`) calls
/// `route_output_stream_chunk`, which calls `route_agent_write`. That takes the generation call
/// lease at once (`lease_call`) and gives a `FilesystemCall` that has not started. The stream
/// keeps the call in `AgentFileOutputState::Waiting` until the guest polls the stream. The lease
/// goes away only when the call completes or is dropped. `wait_for_calls` counts leases, so
/// without a limit the capture waits for a write that nobody drives. Unload does not see this,
/// because it drops the wasm store, with the stream and the lease, before it waits. wasi-libc
/// always uses `blocking-write-and-flush`, so guests built on it do not do this. A hand-written
/// component or a faulty SDK can. The limit turns a stopped agent into a skipped capture. Keep
/// the limit also if the stream later takes its lease at the first poll.
#[allow(dead_code)]
pub(crate) fn capture<Adapter: SandboxFilesystemAdapter>(
    filesystem: &ResidentFilesystem<Adapter>,
    wait: Duration,
) -> impl Future<Output = Result<FilesystemCapture, CaptureError>> + Send + 'static {
    let generation = Arc::clone(
        filesystem
            .generation
            .as_ref()
            .expect("agent filesystem generation already consumed"),
    );
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_module_task(async move {
        let result = complete_capture(&generation, wait).await;
        if let Err(Ok(unobserved)) = sender.send(result)
            && let Err(error) = unobserved.discard().await
        {
            tracing::warn!(error = %error, "Failed to discard an unobserved filesystem capture");
        }
    });
    async move {
        receiver
            .await
            .expect("module-owned filesystem capture stopped unexpectedly")
    }
}

async fn complete_capture<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    wait: Duration,
) -> Result<FilesystemCapture, CaptureError> {
    generation
        .registry
        .begin_transition()
        .map_err(|error| match error {
            AccessError::Transitioning => CaptureError::Busy,
            AccessError::Revoked | AccessError::WrongGeneration | AccessError::NotPermitted => {
                CaptureError::Invalidated
            }
        })?;
    let result = match tokio::time::timeout(wait, generation.registry.wait_for_calls()).await {
        Ok(()) => capture_into_scratch(generation).await,
        Err(_) => Err(CaptureError::Busy),
    };
    generation.registry.finish_transition();
    result
}

async fn capture_into_scratch<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
) -> Result<FilesystemCapture, CaptureError> {
    let sandbox = generation
        .sandbox
        .read()
        .await
        .as_ref()
        .cloned()
        .ok_or(CaptureError::Invalidated)?;
    let state = generation.initial_files.lock().unwrap().clone();
    let read_only_files = golem_read_only_files(sandbox.as_ref(), &state)
        .await
        .map_err(CaptureError::Sandbox)?;
    let directory = HostDirectory::create_in(
        generation.scratch.path(),
        OsStr::new(&uuid::Uuid::new_v4().to_string()),
    )
    .await
    .map_err(CaptureError::Sandbox)?;
    match write_capture(sandbox.as_ref(), &state, read_only_files, directory.path()).await {
        Ok(()) => Ok(FilesystemCapture { directory }),
        Err(error) => {
            if let Err(cleanup) = directory.discard().await {
                tracing::warn!(error = %cleanup, "Failed to discard a failed filesystem capture");
            }
            Err(CaptureError::Sandbox(error))
        }
    }
}

/// Finds the read-only files of the declarations that are still the files that the lifecycle
/// installed at their paths. The result is in path order.
async fn golem_read_only_files<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    state: &InitialFileState,
) -> Result<Box<[RecordedReadOnlyFile]>, FilesystemStorageError> {
    let declarations = state.declarations();
    let candidates = state
        .installed
        .iter()
        .filter_map(|(path, installed)| {
            declarations
                .get(path)
                .filter(|file| file.permissions == AgentFilePermissions::ReadOnly)
                .map(|file| (path.as_ref(), (installed, file.content_hash)))
        })
        .collect::<BTreeMap<&Path, (&InstalledFile, AgentFileContentHash)>>();
    futures::stream::iter(candidates)
        .map(Ok)
        .try_fold(
            (PathReader::default(), Vec::new()),
            |(reader, mut files), (path, (installed, content_hash))| async move {
                let (reader, lookup) = reader.read(sandbox, path).await?;
                if let PathLookup::Found(attributes) = lookup
                    && installed.matches(&attributes)
                {
                    files.push(RecordedReadOnlyFile {
                        path: Box::from(path),
                        content_hash,
                        left_out: attributes.link_count == 1,
                    });
                }
                Ok((reader, files))
            },
        )
        .await
        .map(|(_, files)| files.into_boxed_slice())
}

/// Copies the tree into `directory` and writes the record next to it.
async fn write_capture<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    state: &InitialFileState,
    read_only_files: Box<[RecordedReadOnlyFile]>,
    directory: &HostPath,
) -> Result<(), FilesystemStorageError> {
    let tree = directory.child(OsStr::new(TREE_DIRECTORY))?;
    tokio::fs::create_dir(tree.as_path())
        .await
        .map_err(|error| {
            FilesystemStorageError::io("create filesystem capture tree", tree.as_path(), error)
        })?;
    let excluded = Arc::new(TreeExclusions::new(
        read_only_files
            .iter()
            .filter(|file| file.left_out)
            .map(|file| file.path.to_path_buf()),
    ));
    let link_groups = sandbox
        .copy_contents(SandboxPath::at_root(""), excluded, &tree)
        .await?;
    let record = CaptureRecord {
        initial_files: sorted_declarations(&state.initial),
        provisioned_files: sorted_declarations(&state.provisioned),
        read_only_files,
        link_groups,
    };
    let record_path = directory.child(OsStr::new(RECORD_FILE))?;
    let bytes = serde_json::to_vec(&record).map_err(|error| {
        FilesystemStorageError::io(
            "encode filesystem capture record",
            record_path.as_path(),
            std::io::Error::other(error),
        )
    })?;
    tokio::fs::write(record_path.as_path(), bytes)
        .await
        .map_err(|error| {
            FilesystemStorageError::io(
                "write filesystem capture record",
                record_path.as_path(),
                error,
            )
        })
}

fn sorted_declarations(declarations: &Declarations) -> Box<[InitialAgentFile]> {
    declarations
        .iter()
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .cloned()
        .collect()
}

/// Puts the baseline of a `Reconstructing` filesystem in place. Reconstructing -> Reconstructing.
///
/// Without a restore, the initial files of `prepared` are seeded on the empty tree. With a
/// restore, the restored tree is seeded, the other names of each hard-link group of its record are
/// made, and the initial-file rule runs from the declarations of the record to the declarations
/// of `prepared` and the provisioned declarations of the record. The rule seeds the left-out
/// read-only files that it keeps. With equal declarations, it seeds all of them and changes
/// nothing else. The restore goes into its own directory in the scratch directory, which is
/// discarded at the end. Callers run this once, before replay access is requested. Success enables
/// replay access. Any failure seals the returned filesystem for cleanup.
pub(crate) fn materialize_baseline<
    Adapter: SandboxFilesystemAdapter,
    Restore: RestoreTree + 'static,
>(
    filesystem: ReconstructingFilesystem<Adapter>,
    prepared: PreparedInitialFiles,
    restore: Option<Restore>,
) -> impl Future<Output = Result<ReconstructingFilesystem<Adapter>, ReconstructionFailure<Adapter>>>
+ Send
+ 'static {
    let (generation, stage) = filesystem.into_parts();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_module_task(async move {
        let result = complete_baseline(generation, stage, prepared, restore).await;
        if let Err(unobserved) = sender.send(result) {
            drop(unobserved);
        }
    });
    async move {
        receiver
            .await
            .expect("module-owned baseline materialization stopped unexpectedly")
    }
}

async fn complete_baseline<Adapter: SandboxFilesystemAdapter, Restore: RestoreTree>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    mut stage: Reconstructing,
    prepared: PreparedInitialFiles,
    restore: Option<Restore>,
) -> Result<ReconstructingFilesystem<Adapter>, ReconstructionFailure<Adapter>> {
    if stage.initial_files_materialized || stage.replay_drained {
        return Err(ReconstructionFailure {
            filesystem: sealed_filesystem(generation, stage.meter),
            source: Error::RuntimeInvalidated,
        });
    }
    let sandbox = generation.sandbox.read().await.as_ref().cloned();
    let result = match (sandbox, restore) {
        (None, _) => Err(Error::RuntimeInvalidated),
        (Some(sandbox), None) => {
            install_initial_files(&generation, sandbox.as_ref(), prepared).await
        }
        (Some(sandbox), Some(restore)) => {
            restore_baseline(&generation, sandbox.as_ref(), prepared, restore).await
        }
    };
    match result {
        Ok(state) => {
            *generation.initial_files.lock().unwrap() = state;
            stage.initial_files_materialized = true;
            generation.registry.enable_replay_access();
            Ok(AgentFilesystem {
                generation: Some(generation),
                stage: Some(stage),
            })
        }
        Err(source) => Err(ReconstructionFailure {
            filesystem: sealed_filesystem(generation, stage.meter),
            source,
        }),
    }
}

/// Seeds the initial files of `prepared` on the empty tree.
async fn install_initial_files<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    sandbox: &Adapter,
    prepared: PreparedInitialFiles,
) -> Result<InitialFileState, Error> {
    let (initial, sources) = prepared.into_parts();
    let installed = install(
        generation,
        sandbox,
        sources,
        &Declarations::new(),
        &HashMap::new(),
        &initial,
        &HashMap::new(),
    )
    .await?;
    Ok(InitialFileState {
        initial,
        provisioned: Declarations::new(),
        installed,
    })
}

/// Restores into a new scratch directory, puts the baseline in place, and discards the directory.
///
/// A failure to discard the directory is logged and does not change the result. The next startup
/// cleans the scratch directory.
async fn restore_baseline<Adapter: SandboxFilesystemAdapter, Restore: RestoreTree>(
    generation: &FilesystemGeneration<Adapter>,
    sandbox: &Adapter,
    prepared: PreparedInitialFiles,
    restore: Restore,
) -> Result<InitialFileState, Error> {
    let directory = HostDirectory::create_in(
        generation.scratch.path(),
        OsStr::new(&uuid::Uuid::new_v4().to_string()),
    )
    .await
    .map_err(Error::Sandbox)?;
    let restored = restore_from(generation, sandbox, prepared, restore, directory.path()).await;
    if let Err(cleanup) = directory.discard().await {
        tracing::warn!(error = %cleanup, "Failed to discard a filesystem restore directory");
    }
    restored
}

async fn restore_from<Adapter: SandboxFilesystemAdapter, Restore: RestoreTree>(
    generation: &FilesystemGeneration<Adapter>,
    sandbox: &Adapter,
    prepared: PreparedInitialFiles,
    restore: Restore,
    directory: &HostPath,
) -> Result<InitialFileState, Error> {
    restore
        .restore(directory.as_path())
        .await
        .map_err(|error| Error::Baseline(Box::new(error)))?;
    let CaptureRecord {
        initial_files,
        provisioned_files,
        read_only_files,
        link_groups,
    } = CaptureRecord::read(directory).await?;
    let tree = directory
        .child(OsStr::new(TREE_DIRECTORY))
        .map_err(Error::Sandbox)?;
    seed_with_retry(
        generation,
        sandbox,
        SeedEntry {
            source: tree,
            target: SandboxPath::at_root(""),
            access: SeedAccess::FromSource,
            existing: OnExisting::Fail,
        },
    )
    .await?;
    link_other_names(sandbox, &link_groups).await?;
    let old = InitialFileState {
        initial: declarations_of(
            initial_files.into_vec(),
            "restore unique initial-file declaration",
        )?,
        provisioned: declarations_of(
            provisioned_files.into_vec(),
            "restore unique entity-provisioned file declaration",
        )?,
        installed: restored_installed_files(sandbox, &read_only_files)
            .await
            .map_err(|source| classify_query_error(generation, source))?,
    };
    let (initial, sources) = prepared.into_parts();
    validate_compatible(&old.provisioned, &initial)?;
    let new = merged_declarations(&initial, &old.provisioned);
    let old_declarations = old.declarations();
    let states = observe(sandbox, &old_declarations, &new, &old.installed)
        .await
        .map_err(|source| classify_query_error(generation, source))?
        .into_iter()
        .chain(
            read_only_files
                .iter()
                .filter(|file| file.left_out)
                .map(|file| (file.path.clone(), PathState::LeftOut)),
        )
        .collect();
    let installed = install(
        generation,
        sandbox,
        sources,
        &old_declarations,
        &old.installed,
        &new,
        &states,
    )
    .await?;
    Ok(InitialFileState {
        initial,
        provisioned: old.provisioned,
        installed,
    })
}

/// Makes the other names of each group into hard links to its first name.
async fn link_other_names<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    groups: &[LinkGroup],
) -> Result<(), Error> {
    let names = groups
        .iter()
        .flat_map(|group| {
            group
                .others
                .iter()
                .map(|other| (group.first.as_ref(), other.as_ref()))
        })
        .collect::<Vec<(&Path, &Path)>>();
    futures::stream::iter(names)
        .map(Ok)
        .try_for_each(|(first, other)| async move {
            sandbox
                .hard_link(SandboxPath::at_root(first), SandboxPath::at_root(other))
                .await
                .map_err(Error::Sandbox)
        })
        .await
}

/// Records the read-only files of a record whose bytes are in the restored tree as the files that
/// the lifecycle installed at their paths.
async fn restored_installed_files<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    files: &[RecordedReadOnlyFile],
) -> Result<HashMap<Box<Path>, InstalledFile>, FilesystemStorageError> {
    let with_bytes = files
        .iter()
        .filter(|file| !file.left_out)
        .collect::<Vec<&RecordedReadOnlyFile>>();
    futures::stream::iter(with_bytes)
        .map(Ok)
        .try_fold(
            (PathReader::default(), HashMap::new()),
            |(reader, mut installed), file| async move {
                let (reader, lookup) = reader.read(sandbox, &file.path).await?;
                if let PathLookup::Found(attributes) = lookup
                    && attributes.kind == SandboxObjectKind::File
                {
                    installed.insert(file.path.clone(), InstalledFile::of(&attributes));
                }
                Ok((reader, installed))
            },
        )
        .await
        .map(|(_, installed)| installed)
}
