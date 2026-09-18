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
use futures::StreamExt as _;
use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::ffi::OsStr;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use test_r::{test, timeout};

const NO_RESTORE: Option<Infallible> = None;

/// A restore that fills its directory with a fixture closure.
struct FixtureRestore<F>(F);

impl<F: FnOnce(&Path) -> Result<(), RestoreError> + Send> RestoreTree for FixtureRestore<F> {
    fn restore(self, into: &Path) -> impl Future<Output = Result<(), RestoreError>> + Send {
        std::future::ready((self.0)(into))
    }
}

/// Waits until `condition` holds, and fails the test after five seconds.
async fn eventually(condition: impl Fn() -> bool) {
    assert!(
        holds_within(Duration::from_secs(5), condition).await,
        "the condition did not hold in time"
    );
}

/// Gives true if `condition` holds before `limit` ends.
async fn holds_within(limit: Duration, condition: impl Fn() -> bool) -> bool {
    tokio::time::timeout(limit, async {
        let held = futures::stream::repeat(())
            .then(|()| tokio::task::yield_now())
            .filter(|()| std::future::ready(condition()));
        std::pin::pin!(held).next().await
    })
    .await
    .is_ok()
}

fn file_attributes(object: u64, size: u64, link_count: u64, read_only: bool) -> SandboxAttributes {
    SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count,
        size,
        accessed: None,
        modified: None,
        read_only,
        object: SandboxObjectId::scripted(object),
    }
}

fn scratch_is_empty(scratch: &HostDirectory) -> bool {
    std::fs::read_dir(scratch.path().as_path())
        .unwrap()
        .next()
        .is_none()
}

/// Initial-file content in an in-memory store, with a file loader over it.
struct InitialFileStore {
    environment_id: EnvironmentId,
    service: Arc<InitialAgentFilesService>,
    loader: Arc<FileLoader>,
}

impl InitialFileStore {
    async fn new() -> Self {
        let service = Arc::new(InitialAgentFilesService::new(Arc::new(
            InMemoryBlobStorage::new(),
        )));
        Self {
            environment_id: EnvironmentId::new(),
            loader: Arc::new(FileLoader::new(
                Arc::clone(&service),
                initial_files_directory().await,
            )),
            service,
        }
    }

    async fn declare(
        &self,
        path: &str,
        permissions: AgentFilePermissions,
        content: &[u8],
    ) -> InitialAgentFile {
        let content_hash = self
            .service
            .put_if_not_exists(
                self.environment_id,
                content
                    .to_vec()
                    .map_error(widen_infallible::<anyhow::Error>)
                    .map_item(|item| item.map_err(widen_infallible::<anyhow::Error>)),
            )
            .await
            .unwrap();
        InitialAgentFile {
            content_hash,
            path: AgentFilePath::from_abs_str(path).unwrap(),
            permissions,
            size: content.len() as u64,
        }
    }

    async fn prepare(&self, files: &[InitialAgentFile]) -> PreparedInitialFiles {
        prepare_initial_files(Arc::clone(&self.loader), self.environment_id, files)
            .await
            .unwrap()
    }
}

/// Writes a capture directory with `files` in its tree and `record` next to it.
fn write_capture_directory(into: &Path, files: &[(&str, &[u8])], record: &serde_json::Value) {
    std::fs::create_dir(into.join("tree")).unwrap();
    files.iter().for_each(|(path, content)| {
        let target = into.join("tree").join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, content).unwrap();
    });
    std::fs::write(
        into.join("record.json"),
        serde_json::to_vec(record).unwrap(),
    )
    .unwrap();
}

/// Sets the modification time of the file or directory at `path`.
fn set_modified(path: &Path, modified: std::time::SystemTime) {
    std::fs::File::open(path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
}

fn record(
    initial: &[&InitialAgentFile],
    left_out: serde_json::Value,
    link_groups: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "initial_files": initial,
        "provisioned_files": [],
        "left_out": left_out,
        "link_groups": link_groups,
    })
}

async fn scripted_resident(
    control: &ScriptedSandboxFilesystemControl,
    filesystem: TestAgentFilesystem<Reconstructing>,
    prepared: PreparedInitialFiles,
) -> TestAgentFilesystem<Resident> {
    let filesystem = materialize_baseline(filesystem, prepared, NO_RESTORE)
        .await
        .unwrap();
    let filesystem = finish_replay(filesystem).await.unwrap();
    control.push_observe_allocation(Err(unsupported_allocation()));
    finish_reconstruction(filesystem).await.unwrap()
}

fn scratch_of<Stage: FilesystemStage>(
    filesystem: &TestAgentFilesystem<Stage>,
) -> Arc<HostDirectory> {
    Arc::clone(&filesystem.generation.as_ref().unwrap().scratch)
}

async fn delete_scripted_resident(
    control: &ScriptedSandboxFilesystemControl,
    filesystem: TestAgentFilesystem<Resident>,
) {
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("10s")]
async fn capture_waits_for_a_gated_call_and_reopens_admission() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file_with_access(&generation_handle, &control, 70, AccessMode::Read).await;
    let range = ReadRange {
        offset: 0,
        length: 4,
    };
    control.push_read(Ok(Bytes::from_static(b"data")));
    let gate = control.block("read");
    let reading = tokio::spawn(read_file(&generation_handle, &file, range).unwrap());
    gate.wait_started().await;
    control.push_copy_contents(Ok(Box::new([])));

    let capturing = tokio::spawn(capture(&filesystem, Duration::from_secs(30)));
    eventually(|| {
        matches!(
            read_file(&generation_handle, &file, range).map(drop),
            Err(AccessError::Transitioning)
        )
    })
    .await;

    assert!(
        !holds_within(Duration::from_millis(200), || has_call(
            &control,
            "copy_contents("
        ))
        .await,
        "the capture must not copy the tree while a call runs"
    );
    assert!(!capturing.is_finished());
    gate.release();
    assert_eq!(reading.await.unwrap().unwrap(), Bytes::from_static(b"data"));
    let captured = capturing.await.unwrap().unwrap();
    assert!(has_call(&control, "copy_contents("));
    assert!(captured.directory().join("tree").is_dir());
    assert!(captured.directory().join("record.json").is_file());
    assert!(
        read_file(&generation_handle, &file, range).is_ok(),
        "admission must open again after the capture"
    );
    let scratch = scratch_of(&filesystem);
    captured.discard().await.unwrap();
    assert!(scratch_is_empty(&scratch));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_close(Ok(()));
    close(OpenNode::File(file)).await.unwrap();
    delete_scripted_resident(&control, filesystem).await;
}

#[test]
#[timeout("10s")]
async fn capture_gives_busy_at_the_deadline_and_reopens_admission() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file_with_access(&generation_handle, &control, 71, AccessMode::Read).await;
    let range = ReadRange {
        offset: 0,
        length: 4,
    };
    control.push_read(Ok(Bytes::from_static(b"data")));
    let gate = control.block("read");
    let reading = tokio::spawn(read_file(&generation_handle, &file, range).unwrap());
    gate.wait_started().await;

    let result = capture(&filesystem, Duration::from_millis(50)).await;

    assert!(matches!(result, Err(CaptureError::Busy)));
    assert!(!has_call(&control, "copy_contents("));
    assert!(scratch_is_empty(&scratch_of(&filesystem)));
    assert!(
        read_file(&generation_handle, &file, range).is_ok(),
        "admission must open again after a busy capture"
    );
    gate.release();
    reading.await.unwrap().unwrap();
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_close(Ok(()));
    close(OpenNode::File(file)).await.unwrap();
    delete_scripted_resident(&control, filesystem).await;
}

#[test]
#[timeout("10s")]
async fn capture_waits_for_a_dropped_call_that_still_runs() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file_with_access(&generation_handle, &control, 72, AccessMode::Read).await;
    let range = ReadRange {
        offset: 0,
        length: 4,
    };
    control.push_read(Ok(Bytes::from_static(b"data")));
    let gate = control.block("read");
    let reading = tokio::spawn(read_file(&generation_handle, &file, range).unwrap());
    gate.wait_started().await;
    reading.abort();
    assert!(reading.await.unwrap_err().is_cancelled());
    control.push_copy_contents(Ok(Box::new([])));

    let capturing = tokio::spawn(capture(&filesystem, Duration::from_secs(30)));
    eventually(|| {
        matches!(
            read_file(&generation_handle, &file, range).map(drop),
            Err(AccessError::Transitioning)
        )
    })
    .await;

    assert!(
        !holds_within(Duration::from_millis(200), || has_call(
            &control,
            "copy_contents("
        ))
        .await,
        "the capture must not copy the tree while a dropped call still runs"
    );
    assert!(!capturing.is_finished());
    gate.release();
    let captured = capturing.await.unwrap().unwrap();
    assert!(has_call(&control, "copy_contents("));
    captured.discard().await.unwrap();
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_close(Ok(()));
    close(OpenNode::File(file)).await.unwrap();
    delete_scripted_resident(&control, filesystem).await;
}

#[test]
#[timeout("10s")]
async fn a_deletion_waits_for_a_capture_whose_future_the_caller_dropped() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    let scratch = scratch_of(&filesystem);
    control.push_copy_contents(Ok(Box::new([])));
    let copy = control.block("copy_contents");
    drop(capture(&filesystem, Duration::from_secs(30)));
    copy.wait_started().await;
    control.push_delete_and_verify(Ok(()));

    let deletion = tokio::spawn(delete(seal(filesystem)));

    assert!(
        !holds_within(Duration::from_millis(200), || deletion.is_finished()).await,
        "the deletion must wait for the capture"
    );
    copy.release();
    deletion.await.unwrap().unwrap();
    eventually(|| scratch_is_empty(&scratch)).await;
}

#[test]
async fn capture_leaves_out_the_read_only_files_that_hold_golem_s_file_with_a_single_name() {
    let store = InitialFileStore::new().await;
    let read_only = |path: &'static str| {
        let store = &store;
        async move {
            store
                .declare(path, AgentFilePermissions::ReadOnly, path.as_bytes())
                .await
        }
    };
    let files = [
        read_only("/directory").await,
        read_only("/installed").await,
        read_only("/linked").await,
        read_only("/missing").await,
        read_only("/other-content").await,
        read_only("/other-object").await,
        store
            .declare(
                "/read-write",
                AgentFilePermissions::ReadWrite,
                b"/read-write",
            )
            .await,
        read_only("/resized").await,
        read_only("/writable").await,
    ];
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    files.iter().for_each(|_| control.push_seed(Ok(())));
    files
        .iter()
        .filter(|file| file.permissions == AgentFilePermissions::ReadOnly)
        .zip(1..)
        .for_each(|(file, object)| {
            control.push_get_attributes(Ok(file_attributes(object, file.size, 1, true)))
        });
    let prepared = store.prepare(&files).await;
    let resident = scripted_resident(&control, filesystem, prepared).await;
    let size_of = |path: &str| {
        files
            .iter()
            .find(|file| file.path.to_rel_string() == path)
            .unwrap()
            .size
    };
    [
        Ok(SandboxAttributes {
            kind: SandboxObjectKind::Directory,
            ..file_attributes(1, size_of("directory"), 1, true)
        }),
        Ok(file_attributes(2, size_of("installed"), 1, true)),
        Ok(file_attributes(3, size_of("linked"), 2, true)),
        Err(missing("read a missing initial file")),
        Ok(file_attributes(50, size_of("other-content"), 1, true)),
        Ok(file_attributes(60, size_of("other-object"), 1, true)),
        Ok(file_attributes(7, 1, 1, true)),
        Ok(file_attributes(8, size_of("writable"), 1, false)),
    ]
    .into_iter()
    .for_each(|attributes| control.push_get_attributes(attributes));
    [
        (50, b"/OTHER-content".as_slice()),
        (60, b"/other-object".as_slice()),
    ]
    .into_iter()
    .for_each(|(object, content)| {
        control.push_open(Ok(SandboxOpened::scripted_file(object)));
        control.push_read(Ok(Bytes::from_static(content)));
        control.push_read(Ok(Bytes::new()));
        control.push_close(Ok(()));
    });
    control.push_copy_contents(Ok(Box::new([LinkGroup {
        first: Path::new("linked").into(),
        others: Box::new([Box::from(Path::new("elsewhere"))]),
    }])));

    let captured = capture(&resident, Duration::from_secs(5)).await.unwrap();

    let copy_call = control
        .calls()
        .into_iter()
        .find(|call| call.starts_with("copy_contents("))
        .unwrap();
    assert!(
        copy_call.contains(r#"excluded=["installed", "other-object"]"#),
        "{copy_call}"
    );
    assert_eq!(
        call_count(&control, "open("),
        2,
        "the capture reads the content of a file only when the recorded object does not match"
    );
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(captured.directory().join("record.json")).unwrap())
            .unwrap();
    assert_eq!(
        record,
        serde_json::json!({
            "initial_files": files,
            "provisioned_files": [],
            "left_out": ["installed", "other-object"],
            "link_groups": [{ "first": "linked", "others": ["elsewhere"] }],
        })
    );
    captured.discard().await.unwrap();
    delete_scripted_resident(&control, resident).await;
}

#[test]
async fn baseline_with_a_restore_restores_seeds_the_tree_makes_the_links_then_applies_the_rule() {
    let store = InitialFileStore::new().await;
    let kept = store
        .declare("/kept", AgentFilePermissions::ReadOnly, b"kept")
        .await;
    let notes = store
        .declare("/notes", AgentFilePermissions::ReadWrite, b"notes")
        .await;
    let capture_record = record(
        &[&kept, &notes],
        serde_json::json!(["kept"]),
        serde_json::json!([{ "first": "data/a", "others": ["data/b"] }]),
    );
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    control.push_seed(Ok(()));
    control.push_hard_link(Ok(()));
    control.push_seed(Ok(()));
    control.push_get_attributes(Ok(file_attributes(1, 0, 1, true)));
    control.push_set_times(Ok(()));
    control.push_set_times(Ok(()));
    let root_modified = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_010);
    let data_modified = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_020);
    let restored_into = Arc::new(Mutex::new(None));
    let restore = FixtureRestore({
        let control = control.clone();
        let restored_into = Arc::clone(&restored_into);
        move |into: &Path| {
            assert!(
                !has_call(&control, "seed("),
                "the restore must run before the first seed"
            );
            write_capture_directory(into, &[("data/a", b"linked")], &capture_record);
            set_modified(&into.join("tree/data"), data_modified);
            set_modified(&into.join("tree"), root_modified);
            *restored_into.lock().unwrap() = Some(into.to_path_buf());
            Ok(())
        }
    });
    let scratch = scratch_of(&filesystem);
    let prepared = store.prepare(&[kept.clone(), notes.clone()]).await;

    let filesystem = materialize_baseline(filesystem, prepared, Some(restore))
        .await
        .unwrap();

    let into = restored_into.lock().unwrap().clone().unwrap();
    let calls = control
        .calls()
        .into_iter()
        .filter(|call| !call.starts_with("create_fresh("))
        .collect::<Vec<_>>();
    assert_eq!(
        calls
            .iter()
            .map(|call| call.split_once('(').unwrap().0)
            .collect::<Vec<_>>(),
        [
            "seed",
            "hard_link",
            "seed",
            "get_path_attributes",
            "set_path_times",
            "set_path_times"
        ],
        "{calls:#?}"
    );
    // The root and the directory that got a link get the times of the restored tree, in path order.
    let time_change = |path: &str, modified| {
        format!(
            "path: \"{path}\" }}, follow=No, times={:?}",
            SandboxTimeChanges {
                accessed: SandboxTimeChange::Keep,
                modified: SandboxTimeChange::Set(modified),
            }
        )
    };
    assert!(
        calls[4].contains(&time_change(".", root_modified)),
        "{}",
        calls[4]
    );
    assert!(
        calls[5].contains(&time_change("data", data_modified)),
        "{}",
        calls[5]
    );
    assert!(
        calls[0].contains(&format!(
            "source={}, target=SandboxPath {{ base: Root, path: \"\" }}, access=FromSource, placement=CreateNew",
            into.join("tree").display()
        )),
        "{}",
        calls[0]
    );
    assert!(
        calls[2].contains(r#"path: "kept" }, access=ReadOnly, placement=CreateNew"#),
        "{}",
        calls[2]
    );
    assert_eq!(
        control.sandbox_path_calls(),
        [ScriptedSandboxPathCall::HardLink {
            source: ScriptedSandboxPath {
                base: ScriptedSandboxPathBase::Root,
                path: PathBuf::from("data/a"),
            },
            destination: ScriptedSandboxPath {
                base: ScriptedSandboxPathBase::Root,
                path: PathBuf::from("data/b"),
            },
        }]
    );
    assert!(!into.exists(), "the restore directory must be discarded");
    assert!(scratch_is_empty(&scratch));
    control.push_delete_and_verify(Ok(()));
    delete(abort_reconstruction(filesystem)).await.unwrap();
}

#[test]
async fn a_manual_update_from_a_restore_seeds_the_old_left_out_file_before_the_rule_replaces_it() {
    let store = InitialFileStore::new().await;
    let old = store
        .declare("/config", AgentFilePermissions::ReadOnly, b"old")
        .await;
    let new = store
        .declare("/config", AgentFilePermissions::ReadOnly, b"new")
        .await;
    let old_source = store
        .loader
        .get_source(store.environment_id, old.content_hash, old.size)
        .await
        .unwrap();
    let new_source = store
        .loader
        .get_source(store.environment_id, new.content_hash, new.size)
        .await
        .unwrap();
    let capture_record = serde_json::json!({
        "initial_files": [&old],
        "provisioned_files": [],
        "left_out": ["config"],
        "link_groups": [],
    });
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    control.push_seed(Ok(()));
    control.push_seed(Ok(()));
    control.push_get_attributes(Ok(file_attributes(1, 0, 1, true)));
    control.push_set_times(Ok(()));
    control.push_get_attributes(Ok(file_attributes(1, old.size, 1, true)));
    control.push_seed(Ok(()));
    control.push_get_attributes(Ok(file_attributes(2, 0, 1, true)));
    let prepared = store.prepare(std::slice::from_ref(&new)).await;

    let filesystem = materialize_baseline(
        filesystem,
        prepared,
        Some(FixtureRestore(move |into: &Path| {
            write_capture_directory(into, &[], &capture_record);
            Ok(())
        })),
    )
    .await
    .unwrap();

    let seeds = control
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("seed("))
        .collect::<Box<[_]>>();
    let source_of = |call: &str| {
        call.split_once("source=")
            .and_then(|(_, rest)| rest.split_once(", target="))
            .map(|(source, _)| source.to_string())
    };
    assert_eq!(seeds.len(), 3, "{seeds:#?}");
    assert!(
        seeds[1].contains(r#"path: "config" }, access=ReadOnly, placement=CreateNew"#),
        "{}",
        seeds[1]
    );
    assert_eq!(
        source_of(&seeds[1]),
        Some(old_source.path().as_path().display().to_string())
    );
    assert!(
        seeds[2].contains(r#"path: "config" }, access=ReadOnly, placement=Replace"#),
        "{}",
        seeds[2]
    );
    assert_eq!(
        source_of(&seeds[2]),
        Some(new_source.path().as_path().display().to_string())
    );
    control.push_delete_and_verify(Ok(()));
    delete(abort_reconstruction(filesystem)).await.unwrap();
}

#[test]
async fn a_failed_discard_of_the_restore_directory_keeps_the_baseline_successful() {
    let store = InitialFileStore::new().await;
    let capture_record = record(&[], serde_json::json!([]), serde_json::json!([]));
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    control.push_seed(Ok(()));
    control.push_set_times(Ok(()));
    let restored_into = Arc::new(Mutex::new(None));
    let restore = FixtureRestore({
        let restored_into = Arc::clone(&restored_into);
        move |into: &Path| {
            write_capture_directory(into, &[], &capture_record);
            let locked = into.join("locked");
            std::fs::create_dir(&locked).unwrap();
            std::fs::write(locked.join("inner"), b"inner").unwrap();
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
            *restored_into.lock().unwrap() = Some(into.to_path_buf());
            Ok(())
        }
    });
    let prepared = store.prepare(&[]).await;

    let materialized = materialize_baseline(filesystem, prepared, Some(restore)).await;

    let into = restored_into.lock().unwrap().clone().unwrap();
    let discard_failed = into.join("locked").exists();
    if discard_failed {
        std::fs::set_permissions(into.join("locked"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        std::fs::remove_dir_all(&into).unwrap();
    }
    assert!(
        discard_failed,
        "the locked directory must make the discard fail"
    );
    let filesystem = match materialized {
        Ok(filesystem) => filesystem,
        Err(failure) => panic!(
            "a failed discard must not fail the baseline: {}",
            failure.source
        ),
    };
    control.push_delete_and_verify(Ok(()));
    delete(abort_reconstruction(filesystem)).await.unwrap();
}

#[test]
async fn a_second_baseline_of_one_reconstruction_is_refused() {
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    let filesystem = materialize_baseline(filesystem, no_initial_files().await, NO_RESTORE)
        .await
        .unwrap();

    let failure = materialize_baseline(filesystem, no_initial_files().await, NO_RESTORE)
        .await
        .expect_err("a second baseline of one reconstruction must be refused");

    assert!(
        matches!(failure.source, Error::RuntimeInvalidated),
        "{}",
        failure.source
    );
    control.push_delete_and_verify(Ok(()));
    delete(failure.filesystem).await.unwrap();
}

#[test]
async fn an_update_reads_only_what_the_initial_file_rule_needs() {
    let store = InitialFileStore::new().await;
    let old_directory = store
        .declare("/directory-now", AgentFilePermissions::ReadWrite, b"old-a")
        .await;
    let old_same_size = store
        .declare("/same-size", AgentFilePermissions::ReadWrite, b"old-b")
        .await;
    let new_directory = store
        .declare("/directory-now", AgentFilePermissions::ReadWrite, b"new-a")
        .await;
    let new_same_size = store
        .declare("/same-size", AgentFilePermissions::ReadWrite, b"new-b")
        .await;
    let below_a_file = store
        .declare(
            "/blocked/sub/file.txt",
            AgentFilePermissions::ReadWrite,
            b"below",
        )
        .await;
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    control.push_seed(Ok(()));
    control.push_seed(Ok(()));
    let resident = scripted_resident(
        &control,
        filesystem,
        store
            .prepare(&[old_directory.clone(), old_same_size.clone()])
            .await,
    )
    .await;
    let object = |kind, size| SandboxAttributes {
        kind,
        link_count: 1,
        size,
        accessed: None,
        modified: None,
        read_only: false,
        object: SandboxObjectId::scripted(5),
    };
    let reads_before = call_count(&control, "get_path_attributes(");
    control.push_get_attributes(Ok(object(SandboxObjectKind::File, 0)));
    control.push_get_attributes(Ok(object(SandboxObjectKind::Directory, old_directory.size)));
    control.push_get_attributes(Ok(object(SandboxObjectKind::File, old_same_size.size)));
    // Only the file with the declared size needs a read of its content. It holds other content.
    control.push_open(Ok(SandboxOpened::scripted_file(5)));
    control.push_read(Ok(Bytes::from_static(b"agent")));
    control.push_read(Ok(Bytes::new()));
    control.push_close(Ok(()));
    let seeds_before = call_count(&control, "seed(");

    let updated = update_initial_files(
        &resident_generation_handle(&resident),
        Arc::clone(&store.loader),
        store.environment_id,
        vec![new_directory, new_same_size, below_a_file],
    )
    .unwrap()
    .await;

    assert_eq!(
        updated.as_ref().err().map(ToString::to_string).as_deref(),
        Some(
            "the agent filesystem holds a file above blocked/sub/file.txt, so the initial \
             files of the agent cannot be installed"
        ),
        "the file above blocked/sub/file.txt must be the first conflict: {updated:?}"
    );
    assert_eq!(
        call_count(&control, "get_path_attributes(") - reads_before,
        3,
        "the update must read each path and only the first object above a path that is not a \
         directory"
    );
    assert_eq!(
        call_count(&control, "open("),
        1,
        "the update must read the content only of a file with the declared size"
    );
    assert_eq!(call_count(&control, "seed("), seeds_before);
    delete_scripted_resident(&control, resident).await;
}

#[test]
async fn a_failed_path_read_stops_an_update_before_its_first_change() {
    let store = InitialFileStore::new().await;
    let added = store
        .declare("/added.txt", AgentFilePermissions::ReadWrite, b"added")
        .await;
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    let resident = scripted_resident(&control, filesystem, store.prepare(&[]).await).await;
    control.push_get_attributes(Err(sandbox_error(
        "read an initial-file path",
        std::io::ErrorKind::InvalidInput,
    )));

    let error = update_initial_files(
        &resident_generation_handle(&resident),
        Arc::clone(&store.loader),
        store.environment_id,
        vec![added],
    )
    .unwrap()
    .await
    .unwrap_err();

    assert!(
        !has_call(&control, "seed("),
        "a failed path read must stop the update before its first change: {error}"
    );
    assert!(matches!(error, Error::Sandbox(_)), "{error}");
    assert!(!filesystem_activity(&resident).has_terminal_failure());
    delete_scripted_resident(&control, resident).await;
}

#[test]
#[timeout("60s")]
async fn an_update_with_two_sizes_for_one_content_fails_before_any_change() {
    let agents = UnmanagedAgents::new().await;
    let store = &agents.store;
    let first = store
        .declare(
            "/first.txt",
            AgentFilePermissions::ReadWrite,
            b"same content",
        )
        .await;
    let second = InitialAgentFile {
        path: AgentFilePath::from_abs_str("/second.txt").unwrap(),
        size: first.size + 1,
        ..first.clone()
    };
    let agent = agents.agent("two-sizes");
    let resident = agents.start(&agent, &[], NO_RESTORE).await.unwrap();

    let error = update_initial_files(
        &resident_generation_handle(&resident),
        Arc::clone(&store.loader),
        store.environment_id,
        vec![first, second],
    )
    .unwrap()
    .await
    .unwrap_err();

    assert!(error.to_string().contains("second.txt"), "{error}");
    let root = agents.root(&agent);
    assert!(!root.join("first.txt").exists());
    assert!(!root.join("second.txt").exists());
    assert!(!filesystem_activity(&resident).has_terminal_failure());
    delete(seal(resident)).await.unwrap();
}

#[test]
async fn a_failed_restore_returns_the_sealed_filesystem_with_its_retryable_flag() {
    futures::stream::iter([true, false])
        .for_each(|retryable| async move {
            let store = InitialFileStore::new().await;
            let (filesystem, control, _) =
                bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
            let scratch = scratch_of(&filesystem);

            let failure = materialize_baseline(
                filesystem,
                store.prepare(&[]).await,
                Some(FixtureRestore(move |_: &Path| {
                    Err(RestoreError {
                        retryable,
                        source: anyhow::anyhow!("programmed restore failure"),
                    })
                })),
            )
            .await
            .unwrap_err();

            assert!(
                matches!(&failure.source, Error::Baseline(error) if error.retryable == retryable)
            );
            assert!(!has_call(&control, "seed("));
            assert!(scratch_is_empty(&scratch));
            control.push_delete_and_verify(Ok(()));
            delete(failure.filesystem).await.unwrap();
        })
        .await;
}

#[test]
async fn a_restore_without_a_record_is_a_baseline_failure_that_is_not_retryable() {
    let store = InitialFileStore::new().await;
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;

    let failure = materialize_baseline(
        filesystem,
        store.prepare(&[]).await,
        Some(FixtureRestore(|into: &Path| {
            std::fs::create_dir(into.join("tree")).unwrap();
            Ok(())
        })),
    )
    .await
    .unwrap_err();

    assert!(matches!(&failure.source, Error::Baseline(error) if !error.retryable));
    assert!(!has_call(&control, "seed("));
    control.push_delete_and_verify(Ok(()));
    delete(failure.filesystem).await.unwrap();
}

#[test]
async fn storage_full_while_seeding_a_restored_tree_is_classified_like_initial_file_seeding() {
    let storage_limits = limits(4096, 8);
    let empty_record = record(&[], serde_json::json!([]), serde_json::json!([]));
    let at_limit = {
        let (filesystem, control, entry) =
            bound_reconstructing_with_recovery(ResolvedStorageLimits::Finite(storage_limits), None)
                .await;
        let window = open_resource_usage_window(&filesystem, permit(&entry).await)
            .await
            .unwrap();
        control.push_seed(Err(sandbox_error(
            "seed restored tree",
            std::io::ErrorKind::StorageFull,
        )));
        control.push_observe_allocation(Ok(allocation(storage_limits.allocated_bytes, 1)));
        let store = InitialFileStore::new().await;
        let restore_record = empty_record.clone();
        let failure = materialize_baseline(
            filesystem,
            store.prepare(&[]).await,
            Some(FixtureRestore(move |into: &Path| {
                write_capture_directory(into, &[], &restore_record);
                Ok(())
            })),
        )
        .await
        .unwrap_err();
        close_window(window, Instant::now() + Duration::from_secs(1))
            .await
            .unwrap();
        control.push_delete_and_verify(Ok(()));
        let classified = matches!(failure.source, Error::AgentQuota(_));
        delete(failure.filesystem).await.unwrap();
        classified
    };
    let below_limit = {
        let recovery = ScriptedWriteRecovery::new([FilesystemWriteRecoveryOutcome::Denied]);
        let (filesystem, control, entry) = bound_reconstructing_with_recovery(
            ResolvedStorageLimits::Finite(storage_limits),
            Some(recovery.handle()),
        )
        .await;
        let window = open_resource_usage_window(&filesystem, permit(&entry).await)
            .await
            .unwrap();
        control.push_seed(Err(sandbox_error(
            "seed restored tree",
            std::io::ErrorKind::StorageFull,
        )));
        control.push_observe_allocation(Ok(allocation(512, 1)));
        let store = InitialFileStore::new().await;
        let failure = materialize_baseline(
            filesystem,
            store.prepare(&[]).await,
            Some(FixtureRestore(move |into: &Path| {
                write_capture_directory(into, &[], &empty_record);
                Ok(())
            })),
        )
        .await
        .unwrap_err();
        close_window(window, Instant::now() + Duration::from_secs(1))
            .await
            .unwrap();
        control.push_delete_and_verify(Ok(()));
        let classified = matches!(failure.source, Error::PhysicalCapacity(_));
        delete(failure.filesystem).await.unwrap();
        classified
    };

    assert!(
        at_limit,
        "storage full at the limit must be an agent quota failure"
    );
    assert!(
        below_limit,
        "storage full below the limit must be a physical capacity failure"
    );
}

#[test]
async fn an_initial_file_seed_that_the_sandbox_refuses_with_a_permission_error_fails_with_not_permitted()
 {
    let store = InitialFileStore::new().await;
    // An install of initial files invalidates the generation when a step fails, whatever the
    // error.
    let file = store
        .declare("/refused", AgentFilePermissions::ReadWrite, b"refused")
        .await;
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    control.push_seed(Err(sandbox_error(
        "seed",
        std::io::ErrorKind::PermissionDenied,
    )));
    let failure = materialize_baseline(filesystem, store.prepare(&[file]).await, NO_RESTORE)
        .await
        .unwrap_err();
    assert!(
        matches!(failure.source, Error::Access(AccessError::NotPermitted)),
        "{}",
        failure.source
    );
    assert_eq!(call_count(&control, "seed("), 1);
    assert!(
        failure
            .filesystem
            .generation
            .as_ref()
            .unwrap()
            .registry
            .is_invalidated()
    );
    control.push_delete_and_verify(Ok(()));
    delete(failure.filesystem).await.unwrap();
}

#[test]
async fn a_restored_tree_seed_that_the_sandbox_refuses_with_a_permission_error_fails_with_not_permitted_and_keeps_the_generation()
 {
    let store = InitialFileStore::new().await;
    // The seed of a restored tree runs outside an install, so the refused seed does not
    // invalidate the generation.
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    control.push_seed(Err(sandbox_error(
        "seed",
        std::io::ErrorKind::PermissionDenied,
    )));
    let capture_record = record(&[], serde_json::json!([]), serde_json::json!([]));
    let failure = materialize_baseline(
        filesystem,
        store.prepare(&[]).await,
        Some(FixtureRestore(move |into: &Path| {
            write_capture_directory(into, &[], &capture_record);
            Ok(())
        })),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(failure.source, Error::Access(AccessError::NotPermitted)),
        "{}",
        failure.source
    );
    assert_eq!(call_count(&control, "seed("), 1);
    assert!(
        !failure
            .filesystem
            .generation
            .as_ref()
            .unwrap()
            .registry
            .is_invalidated()
    );
    control.push_delete_and_verify(Ok(()));
    delete(failure.filesystem).await.unwrap();
}

#[test]
async fn a_failed_hard_link_of_a_restored_tree_returns_the_sealed_filesystem() {
    let store = InitialFileStore::new().await;
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    control.push_seed(Ok(()));
    control.push_hard_link(Err(sandbox_error(
        "link restored name",
        std::io::ErrorKind::PermissionDenied,
    )));
    let scratch = scratch_of(&filesystem);
    let capture_record = record(
        &[],
        serde_json::json!([]),
        serde_json::json!([{ "first": "a", "others": ["b"] }]),
    );

    let failure = materialize_baseline(
        filesystem,
        store.prepare(&[]).await,
        Some(FixtureRestore(move |into: &Path| {
            write_capture_directory(into, &[("a", b"a")], &capture_record);
            Ok(())
        })),
    )
    .await
    .unwrap_err();

    assert!(matches!(failure.source, Error::Sandbox(_)));
    assert!(scratch_is_empty(&scratch));
    control.push_delete_and_verify(Ok(()));
    delete(failure.filesystem).await.unwrap();
}

#[test]
async fn a_retry_of_the_restored_tree_seed_removes_what_the_failed_attempt_made() {
    let store = InitialFileStore::new().await;
    let capture_record = record(&[], serde_json::json!([]), serde_json::json!([]));
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    let entry = |name: &str, kind| crate::sandbox_filesystem::SandboxDirectoryEntry {
        name: name.into(),
        kind,
    };
    control.push_seed(Err(sandbox_error(
        "seed restored tree",
        std::io::ErrorKind::WouldBlock,
    )));
    // The root holds a file and a directory with a file from the failed attempt.
    control.push_open(Ok(SandboxOpened::scripted_directory(1)));
    control.push_read_directory(Ok(vec![
        entry("kept.txt", SandboxObjectKind::File),
        entry("data", SandboxObjectKind::Directory),
    ]));
    control.push_close(Ok(()));
    control.push_unlink_file(Ok(()));
    control.push_open(Ok(SandboxOpened::scripted_directory(2)));
    control.push_read_directory(Ok(vec![entry("inner.txt", SandboxObjectKind::File)]));
    control.push_close(Ok(()));
    control.push_unlink_file(Ok(()));
    control.push_remove_directory(Ok(()));
    control.push_seed(Ok(()));
    control.push_set_times(Ok(()));

    let filesystem = materialize_baseline(
        filesystem,
        store.prepare(&[]).await,
        Some(FixtureRestore(move |into: &Path| {
            write_capture_directory(into, &[("kept.txt", b"kept")], &capture_record);
            Ok(())
        })),
    )
    .await
    .unwrap();

    let calls = control
        .calls()
        .into_iter()
        .filter(|call| !call.starts_with("create_fresh("))
        .collect::<Vec<_>>();
    assert_eq!(
        calls
            .iter()
            .map(|call| call.split_once('(').unwrap().0)
            .collect::<Vec<_>>(),
        [
            "seed",
            "open",
            "read_directory",
            "close",
            "unlink_file",
            "open",
            "read_directory",
            "close",
            "unlink_file",
            "remove_directory",
            "seed",
            "set_path_times"
        ],
        "{calls:#?}"
    );
    [
        (1, "."),
        (4, "kept.txt"),
        (5, "data"),
        (8, "data/inner.txt"),
        (9, "data"),
    ]
    .into_iter()
    .for_each(|(index, path)| {
        assert!(
            calls[index].contains(&format!(r#"base: Root, path: "{path}" }}"#)),
            "{}",
            calls[index]
        );
    });
    control.push_delete_and_verify(Ok(()));
    delete(abort_reconstruction(filesystem)).await.unwrap();
}

#[test]
async fn an_update_that_fails_after_the_plan_invalidates_the_generation_and_a_conflict_does_not() {
    let store = InitialFileStore::new().await;
    let old = store
        .declare("/config", AgentFilePermissions::ReadOnly, b"old")
        .await;
    let new = store
        .declare("/config", AgentFilePermissions::ReadOnly, b"new")
        .await;
    let added = store
        .declare("/added", AgentFilePermissions::ReadOnly, b"added")
        .await;

    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    control.push_seed(Ok(()));
    control.push_get_attributes(Ok(file_attributes(1, 0, 1, true)));
    let resident = scripted_resident(
        &control,
        filesystem,
        store.prepare(std::slice::from_ref(&old)).await,
    )
    .await;
    let generation_handle = resident_generation_handle(&resident);
    control.push_get_attributes(Ok(file_attributes(9, 0, 1, false)));

    let conflict = update_initial_files(
        &generation_handle,
        Arc::clone(&store.loader),
        store.environment_id,
        vec![old.clone(), added],
    )
    .unwrap()
    .await
    .unwrap_err();

    assert_eq!(
        conflict.to_string(),
        "the agent filesystem holds another object at added, so the initial files of \
         the agent cannot be installed"
    );
    assert_eq!(call_count(&control, "seed("), 1);
    assert!(!filesystem_activity(&resident).has_terminal_failure());

    control.push_get_attributes(Ok(file_attributes(1, old.size, 1, true)));
    control.push_seed(Err(sandbox_error(
        "replace initial file",
        std::io::ErrorKind::InvalidInput,
    )));
    let failure = update_initial_files(
        &generation_handle,
        Arc::clone(&store.loader),
        store.environment_id,
        vec![new],
    )
    .unwrap()
    .await
    .unwrap_err();

    assert!(matches!(failure, Error::Sandbox(_)), "{failure}");
    assert!(filesystem_activity(&resident).has_terminal_failure());
    delete_scripted_resident(&control, resident).await;
}

#[test]
async fn an_update_of_a_file_that_the_agent_removed_names_the_file_that_is_gone() {
    let store = InitialFileStore::new().await;
    let installed = store
        .declare("/config", AgentFilePermissions::ReadOnly, b"installed")
        .await;
    let changed = store
        .declare("/config", AgentFilePermissions::ReadOnly, b"changed")
        .await;

    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    control.push_seed(Ok(()));
    control.push_get_attributes(Ok(file_attributes(7, installed.size, 1, true)));
    let resident = scripted_resident(
        &control,
        filesystem,
        store.prepare(std::slice::from_ref(&installed)).await,
    )
    .await;
    // The agent removed Golem's file, so nothing is at the path that the update changes.
    control.push_get_attributes(Err(sandbox_error(
        "get sandbox filesystem path attributes",
        std::io::ErrorKind::NotFound,
    )));

    let updated = update_initial_files(
        &resident_generation_handle(&resident),
        Arc::clone(&store.loader),
        store.environment_id,
        vec![changed],
    )
    .unwrap()
    .await;

    match &updated {
        Err(error) => assert_eq!(
            error.to_string(),
            "the agent filesystem no longer holds the file that Golem installed at config, \
             so the initial files of the agent cannot be installed"
        ),
        Ok(()) => panic!("the update must fail with a conflict"),
    }
    assert!(!filesystem_activity(&resident).has_terminal_failure());
    delete_scripted_resident(&control, resident).await;
}

#[test]
async fn an_agent_file_with_the_recorded_object_and_write_bits_is_never_golem_s_file() {
    let store = InitialFileStore::new().await;
    let installed = store
        .declare("/config", AgentFilePermissions::ReadOnly, b"installed")
        .await;
    let changed = store
        .declare("/config", AgentFilePermissions::ReadOnly, b"changed")
        .await;
    let writable = store
        .declare("/config", AgentFilePermissions::ReadWrite, b"installed")
        .await;
    // A name, and the files of an update. Each update must fail with a conflict, because the path
    // does not hold Golem's file of the old declaration.
    let cases: [(&'static str, Vec<InitialAgentFile>); 3] = [
        ("a read-only update", vec![changed]),
        ("a read-write update", vec![writable]),
        ("an update that drops the path", vec![]),
    ];

    futures::stream::iter(cases)
        .for_each(|(name, files)| {
            let store = &store;
            let installed = installed.clone();
            async move {
                let size = installed.size;
                let (filesystem, control, _) =
                    bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None)
                        .await;
                control.push_seed(Ok(()));
                control.push_get_attributes(Ok(file_attributes(7, size, 1, true)));
                let resident =
                    scripted_resident(&control, filesystem, store.prepare(&[installed]).await)
                        .await;
                // The agent removed Golem's file and wrote a new file with the declared size at
                // the path. The new file got the recorded object, and it has write bits. So only
                // the write bits can show that it is not Golem's file.
                control.push_get_attributes(Ok(file_attributes(7, size, 1, false)));
                let changes_before =
                    call_count(&control, "seed(") + call_count(&control, "unlink_file(");
                let opens_before = call_count(&control, "open(");

                let updated = update_initial_files(
                    &resident_generation_handle(&resident),
                    Arc::clone(&store.loader),
                    store.environment_id,
                    files,
                )
                .unwrap()
                .await;

                match &updated {
                    Err(error) => assert_eq!(
                        error.to_string(),
                        "the agent filesystem holds another object at config, so the \
                         initial files of the agent cannot be installed",
                        "{name}"
                    ),
                    Ok(()) => panic!("{name}: the update must fail with a conflict"),
                }
                assert_eq!(
                    call_count(&control, "seed(") + call_count(&control, "unlink_file("),
                    changes_before,
                    "{name}: the update must not change the file of the agent"
                );
                assert_eq!(
                    call_count(&control, "open("),
                    opens_before,
                    "{name}: the write bits alone must show that the file is not Golem's file"
                );
                assert!(
                    !filesystem_activity(&resident).has_terminal_failure(),
                    "{name}"
                );
                delete_scripted_resident(&control, resident).await;
            }
        })
        .await;
}

/// Filesystems on unmanaged storage in a temporary directory, with one initial-file store.
struct UnmanagedAgents {
    parent: tempfile::TempDir,
    provisioning: SandboxFilesystemProvisioning,
    scratch: Arc<HostDirectory>,
    store: InitialFileStore,
    component_id: ComponentId,
}

impl UnmanagedAgents {
    async fn new() -> Self {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = sandbox_provisioning(&FilesystemStorageConfig {
            deterministic_root_dir: Some(parent.path().to_path_buf()),
            ..FilesystemStorageConfig::default()
        })
        .unwrap();
        let scratch = Arc::new(
            HostDirectory::create_at_root(&provisioning, OsStr::new(".scratch"))
                .await
                .unwrap(),
        );
        Self {
            parent,
            provisioning,
            scratch,
            store: InitialFileStore::new().await,
            component_id: ComponentId::new(),
        }
    }

    fn agent(&self, name: &str) -> OwnedAgentId {
        OwnedAgentId::new(
            self.store.environment_id,
            &AgentId::from_agent_name_string(self.component_id, name).unwrap(),
        )
    }

    fn root(&self, agent: &OwnedAgentId) -> PathBuf {
        self.parent
            .path()
            .join(agent.environment_id.to_string())
            .join(agent.agent_id.component_id.to_string())
            .join(agent.agent_id.agent_name_encoded())
    }

    /// Starts a resident filesystem with the baseline of `files` and `restore`.
    async fn start<Restore: RestoreTree + 'static>(
        &self,
        agent: &OwnedAgentId,
        files: &[InitialAgentFile],
        restore: Option<Restore>,
    ) -> Result<ResidentFilesystem, Error> {
        let created = create_fresh(
            self.provisioning.clone(),
            Arc::clone(&self.scratch),
            agent.clone(),
            ResolvedStorageLimits::Unlimited,
        )
        .await
        .unwrap();
        let (account, _) = account();
        let reconstructing = bind_configured_resource_usage_metering(
            created,
            account,
            ResourceUsageMeteringConfig {
                compute: false,
                memory: true,
                filesystem: false,
            },
        )
        .unwrap();
        let prepared = self.store.prepare(files).await;
        match materialize_baseline(reconstructing, prepared, restore).await {
            Ok(reconstructing) => {
                let reconstructing = finish_replay(reconstructing).await.unwrap();
                Ok(finish_reconstruction(reconstructing).await.unwrap())
            }
            Err(failure) => {
                delete(failure.filesystem).await.unwrap();
                Err(failure.source)
            }
        }
    }
}

/// A restore that copies a capture directory, as a store gives it back.
fn copying_restore(
    capture: &FilesystemCapture,
) -> FixtureRestore<impl FnOnce(&Path) -> Result<(), RestoreError> + Send + 'static> {
    let from = capture.directory().to_path_buf();
    FixtureRestore(move |into: &Path| {
        copy_directory_contents(&from, into);
        Ok(())
    })
}

/// Copies what is under `from` into `into` with permissions and modification times. A directory
/// gets its time after its contents, because each object made in it changes its time.
fn copy_directory_contents(from: &Path, into: &Path) {
    std::fs::read_dir(from)
        .unwrap()
        .map(|entry| entry.unwrap())
        .for_each(|entry| {
            let source = entry.path();
            let target = into.join(entry.file_name());
            let metadata = std::fs::symlink_metadata(&source).unwrap();
            let modified = metadata.modified().unwrap();
            if metadata.file_type().is_symlink() {
                std::os::unix::fs::symlink(std::fs::read_link(&source).unwrap(), &target).unwrap();
                fs_set_times::set_symlink_times(
                    &target,
                    None,
                    Some(fs_set_times::SystemTimeSpec::Absolute(modified)),
                )
                .unwrap();
            } else if metadata.is_dir() {
                std::fs::create_dir(&target).unwrap();
                copy_directory_contents(&source, &target);
                std::fs::set_permissions(&target, metadata.permissions()).unwrap();
                set_modified(&target, modified);
            } else {
                std::fs::copy(&source, &target).unwrap();
                set_modified(&target, modified);
            }
        });
}

/// What a tree holds at one path.
#[derive(Debug, Eq, PartialEq)]
enum Node {
    Directory { mode: u32 },
    File { mode: u32, content: Vec<u8> },
    Symlink { target: PathBuf },
}

/// What a tree holds: every path with its object, and the names of each object that is not a
/// directory and has several names.
#[derive(Debug, Eq, PartialEq)]
struct Tree {
    nodes: BTreeMap<String, Node>,
    links: BTreeSet<BTreeSet<String>>,
}

/// Reads the tree under `root`, without times.
fn read_tree(root: &Path) -> Tree {
    let entries = list_entries(root, PathBuf::new(), Vec::new());
    let nodes = entries
        .iter()
        .map(|(path, metadata)| {
            let absolute = root.join(path);
            let mode = metadata.permissions().mode() & 0o7777;
            let node = if metadata.file_type().is_symlink() {
                Node::Symlink {
                    target: std::fs::read_link(&absolute).unwrap(),
                }
            } else if metadata.is_dir() {
                Node::Directory { mode }
            } else {
                Node::File {
                    mode,
                    content: std::fs::read(&absolute).unwrap(),
                }
            };
            (path.clone(), node)
        })
        .collect();
    let links = entries
        .iter()
        .filter(|(_, metadata)| !metadata.is_dir() && metadata.nlink() > 1)
        .fold(
            BTreeMap::<(u64, u64), BTreeSet<String>>::new(),
            |mut groups, (path, metadata)| {
                groups
                    .entry((metadata.dev(), metadata.ino()))
                    .or_default()
                    .insert(path.clone());
                groups
            },
        )
        .into_values()
        .collect();
    Tree { nodes, links }
}

fn list_entries(
    root: &Path,
    relative: PathBuf,
    found: Vec<(String, std::fs::Metadata)>,
) -> Vec<(String, std::fs::Metadata)> {
    std::fs::read_dir(root.join(&relative))
        .unwrap()
        .map(|entry| entry.unwrap())
        .fold(found, |mut found, entry| {
            let path = relative.join(entry.file_name());
            let metadata = std::fs::symlink_metadata(root.join(&path)).unwrap();
            let directory = metadata.is_dir();
            found.push((path.to_string_lossy().into_owned(), metadata));
            if directory {
                list_entries(root, path, found)
            } else {
                found
            }
        })
}

#[test]
#[timeout("60s")]
async fn a_read_only_file_with_the_declared_content_that_the_agent_moves_onto_the_path_is_golem_s_file()
 {
    let agents = UnmanagedAgents::new().await;
    let store = &agents.store;
    let ro = AgentFilePermissions::ReadOnly;
    let old = [
        store.declare("/h.txt", ro, b"two").await,
        store.declare("/e/f.txt", ro, b"two").await,
    ];
    let changed = store.declare("/h.txt", ro, b"three").await;
    // A name, the new declarations, and the content that the update leaves at h.txt.
    type MovedFileCase = (&'static str, Vec<InitialAgentFile>, Option<&'static [u8]>);
    let cases: [MovedFileCase; 2] = [
        ("an update that drops h.txt", vec![old[1].clone()], None),
        (
            "an update that changes h.txt",
            vec![changed, old[1].clone()],
            Some(b"three".as_slice()),
        ),
    ];
    let move_onto_the_path =
        |root: &Path| std::fs::rename(root.join("e/f.txt"), root.join("h.txt")).unwrap();

    futures::stream::iter(cases.into_iter().enumerate())
        .for_each(|(index, (name, new, expected))| {
            let agents = &agents;
            let old = &old;
            async move {
                let automatic_agent = agents.agent(&format!("automatic-{index}"));
                let automatic = agents
                    .start(&automatic_agent, old, NO_RESTORE)
                    .await
                    .unwrap();
                move_onto_the_path(&agents.root(&automatic_agent));
                let updated = update_initial_files(
                    &resident_generation_handle(&automatic),
                    Arc::clone(&agents.store.loader),
                    agents.store.environment_id,
                    new.clone(),
                )
                .unwrap()
                .await;
                assert!(updated.is_ok(), "{name}: {updated:?}");
                let automatic_tree = read_tree(&agents.root(&automatic_agent));

                let source_agent = agents.agent(&format!("manual-source-{index}"));
                let source = agents.start(&source_agent, old, NO_RESTORE).await.unwrap();
                move_onto_the_path(&agents.root(&source_agent));
                let captured = capture(&source, Duration::from_secs(5)).await.unwrap();
                let manual_agent = agents.agent(&format!("manual-{index}"));
                let manual = agents
                    .start(&manual_agent, &new, Some(copying_restore(&captured)))
                    .await
                    .unwrap();

                assert_eq!(
                    read_tree(&agents.root(&manual_agent)),
                    automatic_tree,
                    "{name}"
                );
                let at_path = match automatic_tree.nodes.get("h.txt") {
                    Some(Node::File { mode, content, .. }) => {
                        Some((mode & 0o222 == 0, content.as_slice()))
                    }
                    _ => None,
                };
                assert_eq!(at_path, expected.map(|content| (true, content)), "{name}");
                assert!(!automatic_tree.nodes.contains_key("e/f.txt"), "{name}");
                captured.discard().await.unwrap();
                delete(seal(automatic)).await.unwrap();
                delete(seal(source)).await.unwrap();
                delete(seal(manual)).await.unwrap();
            }
        })
        .await;
}

#[test]
#[timeout("60s")]
async fn a_hard_link_of_a_directory_gives_not_permitted_and_later_calls_still_work() {
    let agents = UnmanagedAgents::new().await;
    let agent = agents.agent("directory-link");
    let resident = agents.start(&agent, &[], NO_RESTORE).await.unwrap();
    let generation_handle = resident_generation_handle(&resident);
    let at = |path: &str| PathTarget::at_root(&generation_handle, path).unwrap();
    edit_namespace(
        &generation_handle,
        NamespaceEdit::Insert {
            destination: at("directory"),
            object: NewObject::Directory,
        },
    )
    .unwrap()
    .await
    .unwrap();

    let linked = edit_namespace(
        &generation_handle,
        NamespaceEdit::Link {
            source: at("directory"),
            destination: at("alias"),
        },
    )
    .unwrap()
    .await;

    assert!(
        matches!(linked, Err(Error::Access(AccessError::NotPermitted))),
        "{linked:?}"
    );
    assert!(!filesystem_activity(&resident).has_terminal_failure());
    edit_namespace(
        &generation_handle,
        NamespaceEdit::Insert {
            destination: at("directory/after"),
            object: NewObject::Directory,
        },
    )
    .unwrap()
    .await
    .unwrap();
    let root = agents.root(&agent);
    assert!(root.join("directory/after").is_dir());
    assert!(!root.join("alias").exists());
    delete(seal(resident)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn a_file_that_an_agent_creates_through_the_lifecycle_has_write_bits() {
    let agents = UnmanagedAgents::new().await;
    let agent = agents.agent("created-file");
    let resident = agents.start(&agent, &[], NO_RESTORE).await.unwrap();
    let generation_handle = resident_generation_handle(&resident);

    let created = open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, "created.txt").unwrap(),
        OpenOptions::File {
            access: AccessMode::Write,
            disposition: FileDisposition::CreateExclusive,
            follow: Follow::No,
        },
    )
    .unwrap()
    .await
    .unwrap();
    close(created.node).await.unwrap();

    let permissions = std::fs::metadata(agents.root(&agent).join("created.txt"))
        .unwrap()
        .permissions();
    assert!(
        !permissions.readonly(),
        "a file that an agent creates must have write bits, and its mode is {:o}",
        permissions.mode()
    );
    delete(seal(resident)).await.unwrap();
}

/// The directory places of the path space of the restore property. A directory move renames only
/// between these places. An install or a write through a symlink can also put a file at one of them.
const DIRECTORY_PLACES: [&str; 2] = ["d", "e"];

/// The file places of the path space: three names at the root and the three names in each directory.
/// Regular files and symlinks are at these places, and a step can make a directory at the places
/// named `h`. A file move renames the object at its source, so it can also move such a directory.
const FILE_PLACES: [&str; 9] = ["f", "g", "h", "d/f", "d/g", "d/h", "e/f", "e/g", "e/h"];

/// The places that a hard link takes its source from: the file places, then the directory places.
/// So a step can make a hard link of a directory.
const LINKED_PLACES: [&str; 11] = [
    "f", "g", "h", "d/f", "d/g", "d/h", "e/f", "e/g", "e/h", "d", "e",
];

/// The places where a step makes or removes a directory: the directory places and the file places
/// named `h`. So a directory can be at the path of a declaration.
const NEW_DIRECTORY_PLACES: [&str; 5] = ["d", "e", "h", "d/h", "e/h"];

/// The paths of component declarations. The directory place `d` is also a declared path, so a
/// revision can declare a path under a path that an earlier revision declared. Entity provisioning
/// never uses these paths.
const DECLARED_PLACES: [&str; 7] = ["d", "f", "h", "d/f", "d/h", "e/f", "e/h"];

/// The entity-provisioned files that a history can provision. Each path has one declaration, so two
/// provisionings never declare one path in two ways.
const PROVISIONED_FILES: [DeclaredFile; 3] = [
    DeclaredFile {
        path: "g",
        read_only: true,
        content: 0,
    },
    DeclaredFile {
        path: "d/g",
        read_only: true,
        content: 2,
    },
    DeclaredFile {
        path: "e/g",
        read_only: false,
        content: 1,
    },
];

/// The contents of declarations and writes. Two of them have the same size.
const CONTENTS: [&[u8]; 3] = [b"one", b"two", b"three"];

/// The targets of new symlinks. No target is outside the root. No target is a place where a symlink
/// can be, so no symlink loop occurs.
const SYMLINK_TARGETS: [&str; 4] = ["missing", "d", "d/new", "e"];

/// The number of histories that the restore property checks when `PROPTEST_CASES` is not set.
const RESTORE_PROPERTY_CASES: u32 = 2048;

/// The seed of the restore property. Each run checks the same histories.
const RESTORE_PROPERTY_SEED: [u8; 32] = *b"filesystem-restore-equals-replay";

/// One initial file that a history declares, at a path relative to the root.
#[derive(Clone, Debug)]
struct DeclaredFile {
    path: &'static str,
    read_only: bool,
    content: usize,
}

/// One step of a history: a filesystem operation of the agent, a component update, or an entity
/// provisioning. A file place is an index into `FILE_PLACES`, and the source of a hard link is an
/// index into `LINKED_PLACES`. A moved directory is an index into `DIRECTORY_PLACES`, and a made or
/// removed directory is an index into `NEW_DIRECTORY_PLACES`. A provisioning gives indices into
/// `PROVISIONED_FILES`.
#[derive(Clone, Debug)]
enum HistoryStep {
    Write { file: usize, content: usize },
    Truncate { file: usize, size: u64 },
    SetTimes { file: usize },
    RemoveFile { file: usize },
    MoveFile { source: usize, destination: usize },
    MoveDirectory { source: usize, destination: usize },
    HardLink { source: usize, destination: usize },
    Symlink { file: usize, target: usize },
    CreateDirectory { directory: usize },
    RemoveDirectory { directory: usize },
    Update { files: Box<[DeclaredFile]> },
    Provision { files: Box<[usize]> },
}

/// A history of one agent: its first declarations, its steps, and the number of steps before the
/// capture.
#[derive(Clone, Debug)]
struct History {
    initial: Box<[DeclaredFile]>,
    steps: Box<[HistoryStep]>,
    capture: usize,
}

/// A move of a read-only file of the first declarations away before the capture, and back after
/// it. `file` selects the file among the read-only files that the move can take, and `destination`
/// is an index into `FILE_PLACES`.
#[derive(Clone, Debug)]
enum RoundTrip {
    /// Renames the file to the place `destination`, and back.
    File {
        file: proptest::sample::Index,
        destination: usize,
    },
    /// Renames the directory place above the file to the other directory place, and back.
    Directory { file: proptest::sample::Index },
    /// Links the file to the place `destination` and removes its first name. After the capture,
    /// links the file back to its first name.
    Link {
        file: proptest::sample::Index,
        destination: usize,
    },
}

impl RoundTrip {
    /// Gives the steps before the capture and the step after it. Gives `None` when `initial` has no
    /// read-only file that the round trip can move.
    fn steps(&self, initial: &[DeclaredFile]) -> Option<(Box<[HistoryStep]>, HistoryStep)> {
        match self {
            Self::File { file, destination } => {
                let (source, destination) =
                    Self::file_and_destination(initial, file, *destination)?;
                let away: Box<[HistoryStep]> = Box::new([HistoryStep::MoveFile {
                    source,
                    destination,
                }]);
                let back = HistoryStep::MoveFile {
                    source: destination,
                    destination: source,
                };
                Some((away, back))
            }
            Self::Link { file, destination } => {
                let (source, destination) =
                    Self::file_and_destination(initial, file, *destination)?;
                let away: Box<[HistoryStep]> = Box::new([
                    HistoryStep::HardLink {
                        source,
                        destination,
                    },
                    HistoryStep::RemoveFile { file: source },
                ]);
                let back = HistoryStep::HardLink {
                    source: destination,
                    destination: source,
                };
                Some((away, back))
            }
            Self::Directory { file } => {
                let directories = initial
                    .iter()
                    .filter(|declared| declared.read_only)
                    .filter_map(|declared| declared.path.split_once('/'))
                    .filter_map(|(directory, _)| {
                        DIRECTORY_PLACES
                            .iter()
                            .position(|place| *place == directory)
                    })
                    .collect::<Box<[usize]>>();
                let source = (!directories.is_empty()).then(|| *file.get(&directories))?;
                let destination = (source + 1) % DIRECTORY_PLACES.len();
                let away: Box<[HistoryStep]> = Box::new([HistoryStep::MoveDirectory {
                    source,
                    destination,
                }]);
                let back = HistoryStep::MoveDirectory {
                    source: destination,
                    destination: source,
                };
                Some((away, back))
            }
        }
    }

    /// Gives the file place of the read-only file of `initial` that `file` selects, and another
    /// file place: `destination`, or the next file place when `destination` is the place of the
    /// file.
    fn file_and_destination(
        initial: &[DeclaredFile],
        file: &proptest::sample::Index,
        destination: usize,
    ) -> Option<(usize, usize)> {
        let places = initial
            .iter()
            .filter(|declared| declared.read_only)
            .filter_map(|declared| FILE_PLACES.iter().position(|place| *place == declared.path))
            .collect::<Box<[usize]>>();
        let source = (!places.is_empty()).then(|| *file.get(&places))?;
        let destination = if destination == source {
            (destination + 1) % FILE_PLACES.len()
        } else {
            destination
        };
        Some((source, destination))
    }
}

/// The step results that another agent must give, and the final trees of the replay and of the
/// reference model.
struct Expected<'a> {
    outcomes: &'a [StepOutcome],
    tree: &'a Tree,
    model_tree: &'a Tree,
}

/// What a step gave. An error keeps only the facts that do not name a host path. A conflict keeps
/// the conflicting path.
#[derive(Clone, Debug, Eq, PartialEq)]
enum StepOutcome {
    Done,
    Access(AccessError),
    Sandbox(Option<std::io::ErrorKind>),
    AgentQuota,
    PhysicalCapacity,
    Baseline,
    RuntimeInvalidated,
    Conflict(String),
}

/// Generates the declarations of one revision. A revision never declares a path under another path
/// that it declares.
fn declared_files() -> impl proptest::strategy::Strategy<Value = Box<[DeclaredFile]>> {
    use proptest::strategy::Strategy as _;
    let places: &'static [&'static str] = &DECLARED_PLACES;
    proptest::collection::btree_map(
        proptest::sample::select(places),
        (proptest::arbitrary::any::<bool>(), 0..CONTENTS.len()),
        0..6,
    )
    .prop_map(|files| {
        files
            .iter()
            .filter(|(path, _)| {
                !files
                    .keys()
                    .any(|other| path.starts_with(&format!("{other}/")))
            })
            .map(|(path, (read_only, content))| DeclaredFile {
                path,
                read_only: *read_only,
                content: *content,
            })
            .collect()
    })
}

fn history_step() -> impl proptest::strategy::Strategy<Value = HistoryStep> {
    use proptest::strategy::Strategy as _;
    let file = || 0..FILE_PLACES.len();
    let directory = || 0..DIRECTORY_PLACES.len();
    let new_directory = || 0..NEW_DIRECTORY_PLACES.len();
    proptest::prop_oneof![
        3 => (file(), 0..CONTENTS.len())
            .prop_map(|(file, content)| HistoryStep::Write { file, content }),
        1 => (file(), 0_u64..3).prop_map(|(file, size)| HistoryStep::Truncate { file, size }),
        1 => file().prop_map(|file| HistoryStep::SetTimes { file }),
        2 => file().prop_map(|file| HistoryStep::RemoveFile { file }),
        2 => (file(), file())
            .prop_map(|(source, destination)| HistoryStep::MoveFile { source, destination }),
        2 => (directory(), directory())
            .prop_map(|(source, destination)| HistoryStep::MoveDirectory { source, destination }),
        2 => (0..LINKED_PLACES.len(), file())
            .prop_map(|(source, destination)| HistoryStep::HardLink { source, destination }),
        2 => (file(), 0..SYMLINK_TARGETS.len())
            .prop_map(|(file, target)| HistoryStep::Symlink { file, target }),
        2 => new_directory().prop_map(|directory| HistoryStep::CreateDirectory { directory }),
        1 => new_directory().prop_map(|directory| HistoryStep::RemoveDirectory { directory }),
        3 => declared_files().prop_map(|files| HistoryStep::Update { files }),
        1 => proptest::collection::btree_set(0..PROVISIONED_FILES.len(), 1..=PROVISIONED_FILES.len())
            .prop_map(|files| HistoryStep::Provision { files: files.into_iter().collect() }),
    ]
}

fn round_trip() -> impl proptest::strategy::Strategy<Value = RoundTrip> {
    use proptest::strategy::Strategy as _;
    let file = || proptest::arbitrary::any::<proptest::sample::Index>();
    proptest::prop_oneof![
        (file(), 0..FILE_PLACES.len())
            .prop_map(|(file, destination)| RoundTrip::File { file, destination }),
        file().prop_map(|file| RoundTrip::Directory { file }),
        (file(), 0..FILE_PLACES.len())
            .prop_map(|(file, destination)| RoundTrip::Link { file, destination }),
    ]
}

/// Generates the declarations of a revision that changes `initial`. Each declaration of `initial`
/// stays, gets a content, gets the other permission, or goes. The revision can also declare other
/// paths. A revision never declares a path under another path that it declares.
fn revised_files(
    initial: Box<[DeclaredFile]>,
) -> impl proptest::strategy::Strategy<Value = Box<[DeclaredFile]>> {
    use proptest::strategy::Strategy as _;
    let count = initial.len();
    (
        proptest::collection::vec((0..5_u8, 0..CONTENTS.len()), count),
        declared_files(),
    )
        .prop_map(move |(changes, added)| {
            let files = initial
                .iter()
                .zip(changes)
                .filter_map(|(file, (change, content))| match change {
                    0 | 1 => Some(file.clone()),
                    2 => Some(DeclaredFile {
                        content,
                        ..file.clone()
                    }),
                    3 => Some(DeclaredFile {
                        read_only: !file.read_only,
                        ..file.clone()
                    }),
                    _ => None,
                })
                .chain(added.iter().cloned())
                .fold(BTreeMap::new(), |mut files, file| {
                    files.entry(file.path).or_insert(file);
                    files
                });
            files
                .values()
                .filter(|file| {
                    !files
                        .keys()
                        .any(|other| file.path.starts_with(&format!("{other}/")))
                })
                .cloned()
                .collect()
        })
}

/// Gives the steps that bring the old object of a read-only file of `initial` back to its path
/// after an update replaced the file. The steps are: a hard link of the file to the file place
/// `destination`; an update that gives the path the other content of the same size; a move of the
/// link back onto the path; an update that changes the declaration of the path once more. `file`
/// selects the file among the read-only files of `initial` whose content has the size of another
/// content. `change` selects the last update: a content of another size, the other permission, or
/// no declaration. Gives no step when `initial` has no such file.
fn old_object_back_steps(
    initial: &[DeclaredFile],
    file: &proptest::sample::Index,
    destination: usize,
    change: u8,
) -> Vec<HistoryStep> {
    let candidates = initial
        .iter()
        .filter(|declared| declared.read_only && declared.content < 2)
        .filter_map(|declared| {
            FILE_PLACES
                .iter()
                .position(|place| *place == declared.path)
                .map(|place| (place, declared))
        })
        .collect::<Box<[_]>>();
    if candidates.is_empty() {
        return Vec::new();
    }
    let &(source, declared) = file.get(&candidates);
    let destination = if destination == source {
        (destination + 1) % FILE_PLACES.len()
    } else {
        destination
    };
    let replaced = DeclaredFile {
        content: 1 - declared.content,
        ..declared.clone()
    };
    let revision = |change: Option<DeclaredFile>| {
        initial
            .iter()
            .filter(|other| other.path != declared.path)
            .cloned()
            .chain(change)
            .collect::<Box<[DeclaredFile]>>()
    };
    let last = match change {
        0 => Some(DeclaredFile {
            content: 2,
            ..replaced.clone()
        }),
        1 => Some(DeclaredFile {
            read_only: false,
            ..replaced.clone()
        }),
        _ => None,
    };
    vec![
        HistoryStep::HardLink {
            source,
            destination,
        },
        HistoryStep::Update {
            files: revision(Some(replaced)),
        },
        HistoryStep::MoveFile {
            source: destination,
            destination: source,
        },
        HistoryStep::Update {
            files: revision(last),
        },
    ]
}

/// Generates a few steps of a history whose first declarations are `initial`: a step of
/// [`history_step`], an update to a revision of `initial`, a removal of a file followed by a new
/// file, a new directory or a new symlink at the same place, a write through a new symlink to the
/// directory place `d`, which no file step names, or the steps of [`old_object_back_steps`].
fn history_steps(
    initial: Box<[DeclaredFile]>,
) -> impl proptest::strategy::Strategy<Value = Vec<HistoryStep>> {
    use proptest::strategy::Strategy as _;
    // The places that are file places and also places where a step makes a directory.
    let file_and_directory_places: &'static [&'static str] = &["h", "d/h", "e/h"];
    // The file places at the root, where a symlink with the target `d` names the place `d`.
    let root_file_places: &'static [&'static str] = &["f", "g", "h"];
    let position = |places: &[&str], place: &str| {
        places
            .iter()
            .position(|candidate| *candidate == place)
            .expect("the place is in the list of places")
    };
    let linked = initial.clone();
    proptest::prop_oneof![
        7 => history_step().prop_map(|step| vec![step]),
        2 => revised_files(initial).prop_map(|files| vec![HistoryStep::Update { files }]),
        1 => (
            proptest::arbitrary::any::<proptest::sample::Index>(),
            0..FILE_PLACES.len(),
            0..3_u8,
        )
            .prop_map(move |(file, destination, change)| {
                old_object_back_steps(&linked, &file, destination, change)
            }),
        2 => (0..FILE_PLACES.len(), 0..CONTENTS.len()).prop_map(|(file, content)| {
            vec![
                HistoryStep::RemoveFile { file },
                HistoryStep::Write { file, content },
            ]
        }),
        1 => proptest::sample::select(file_and_directory_places).prop_map(move |place| {
            vec![
                HistoryStep::RemoveFile {
                    file: position(&FILE_PLACES, place),
                },
                HistoryStep::CreateDirectory {
                    directory: position(&NEW_DIRECTORY_PLACES, place),
                },
            ]
        }),
        1 => (0..FILE_PLACES.len(), 0..SYMLINK_TARGETS.len()).prop_map(|(file, target)| {
            vec![
                HistoryStep::RemoveFile { file },
                HistoryStep::Symlink { file, target },
            ]
        }),
        1 => (proptest::sample::select(root_file_places), 0..CONTENTS.len()).prop_map(
            move |(place, content)| {
                let file = position(&FILE_PLACES, place);
                vec![
                    HistoryStep::RemoveFile { file },
                    HistoryStep::Symlink {
                        file,
                        target: position(&SYMLINK_TARGETS, "d"),
                    },
                    HistoryStep::Write { file, content },
                ]
            }
        ),
    ]
}

/// Generates histories. About half of them move a read-only file of the first declarations, or the
/// directory above one, away just before the capture and back just after it. Most of the others
/// update to a revision of the first declarations just after the capture, so that a manual update
/// from a restore meets the changes of the steps before the capture.
fn histories() -> impl proptest::strategy::Strategy<Value = History> {
    use proptest::strategy::Strategy as _;
    declared_files()
        .prop_flat_map(|initial| {
            (
                proptest::strategy::Just(initial.clone()),
                proptest::collection::vec(history_steps(initial.clone()), 0..14),
                proptest::arbitrary::any::<proptest::sample::Index>(),
                proptest::option::of(round_trip()),
                proptest::option::weighted(0.8, revised_files(initial)),
            )
        })
        .prop_map(|(initial, steps, capture, round_trip, revision)| {
            let steps = steps.into_iter().flatten().collect::<Vec<_>>();
            let capture = capture.index(steps.len() + 1);
            match (
                round_trip.and_then(|round_trip| round_trip.steps(&initial)),
                revision,
            ) {
                (None, None) => History {
                    initial,
                    steps: steps.into_boxed_slice(),
                    capture,
                },
                (None, Some(files)) => {
                    let (before, after) = steps.split_at(capture);
                    History {
                        steps: before
                            .iter()
                            .cloned()
                            .chain(std::iter::once(HistoryStep::Update { files }))
                            .chain(after.iter().cloned())
                            .collect(),
                        initial,
                        capture,
                    }
                }
                (Some((away, back)), _) => {
                    let (before, after) = steps.split_at(capture);
                    History {
                        capture: capture + away.len(),
                        steps: before
                            .iter()
                            .cloned()
                            .chain(away.into_vec())
                            .chain(std::iter::once(back))
                            .chain(after.iter().cloned())
                            .collect(),
                        initial,
                    }
                }
            }
        })
}

async fn declare_files(store: &InitialFileStore, files: &[DeclaredFile]) -> Vec<InitialAgentFile> {
    futures::stream::iter(files)
        .then(|file| async move {
            let permissions = if file.read_only {
                AgentFilePermissions::ReadOnly
            } else {
                AgentFilePermissions::ReadWrite
            };
            store
                .declare(
                    &format!("/{}", file.path),
                    permissions,
                    CONTENTS[file.content],
                )
                .await
        })
        .collect()
        .await
}

/// Gives the entity-provisioned files at the indices `files` of `PROVISIONED_FILES`.
fn provisioned_files(files: &[usize]) -> Box<[DeclaredFile]> {
    files
        .iter()
        .map(|index| PROVISIONED_FILES[*index].clone())
        .collect()
}

/// Gives the component declarations that are current after `steps`: the files of the last update
/// that was done, or `initial`.
fn declarations_at(
    initial: &[DeclaredFile],
    steps: &[HistoryStep],
    outcomes: &[StepOutcome],
) -> Box<[DeclaredFile]> {
    steps
        .iter()
        .zip(outcomes)
        .fold(Box::from(initial), |current, step| match step {
            (HistoryStep::Update { files }, StepOutcome::Done) => files.clone(),
            _ => current,
        })
}

/// Gives the outcome of a failed step. This is the only function that recognizes a conflict.
fn error_outcome(error: Error) -> StepOutcome {
    match error {
        Error::Access(error) => StepOutcome::Access(error),
        Error::Sandbox(error) => StepOutcome::Sandbox(error.io_kind()),
        Error::InitialFileConflict(conflict) => {
            StepOutcome::Conflict(conflict.path().display().to_string())
        }
        Error::AgentQuota(_) => StepOutcome::AgentQuota,
        Error::PhysicalCapacity(_) => StepOutcome::PhysicalCapacity,
        Error::Baseline(_) => StepOutcome::Baseline,
        Error::RuntimeInvalidated => StepOutcome::RuntimeInvalidated,
    }
}

fn outcome_of(result: Result<(), Error>) -> StepOutcome {
    result.map_or_else(error_outcome, |()| StepOutcome::Done)
}

/// Writes `content` to the file at `target`, as a guest write through a new descriptor does.
async fn write_file(
    generation_handle: &FilesystemGenerationHandle,
    target: Result<PathTarget, AccessError>,
    content: &'static [u8],
) -> StepOutcome {
    let options = OpenOptions::File {
        access: AccessMode::Write,
        disposition: FileDisposition::CreateOrTruncate,
        follow: Follow::Yes,
    };
    let opened = match target.and_then(|target| open(generation_handle, target, options)) {
        Ok(call) => call.await,
        Err(error) => return StepOutcome::Access(error),
    };
    let opened = match opened {
        Ok(opened) => opened,
        Err(error) => return error_outcome(error),
    };
    let written = match &opened.node {
        OpenNode::File(file) => match write(
            generation_handle,
            file,
            WritePlacement::At(0),
            Bytes::from_static(content),
        ) {
            Ok(call) => call.await.map(drop),
            Err(error) => Err(Error::Access(error)),
        },
        OpenNode::Directory(_) => Ok(()),
    };
    let closed = close(opened.node).await;
    outcome_of(written.and(closed))
}

/// Sets the size of the file at `target`, as a guest set-size through a new descriptor does.
async fn truncate_file(
    generation_handle: &FilesystemGenerationHandle,
    target: Result<PathTarget, AccessError>,
    size: u64,
) -> StepOutcome {
    let options = OpenOptions::Existing {
        expected: ObjectKind::File,
        access: AccessMode::Write,
        follow: Follow::Yes,
    };
    let opened = match target.and_then(|target| open(generation_handle, target, options)) {
        Ok(call) => call.await,
        Err(error) => return StepOutcome::Access(error),
    };
    let opened = match opened {
        Ok(opened) => opened,
        Err(error) => return error_outcome(error),
    };
    let changes = AttributeChanges::File {
        size,
        times: TimeChanges {
            accessed: TimeChange::Keep,
            modified: TimeChange::Keep,
        },
    };
    let resized = match set_attributes(generation_handle, Target::Open(&opened.node), changes) {
        Ok(call) => call.await,
        Err(error) => Err(Error::Access(error)),
    };
    let closed = close(opened.node).await;
    outcome_of(resized.and(closed))
}

/// Sets the access and modification times of the object at `target` to now, as a guest set-times
/// through a path that follows a symlink does.
async fn set_times_now(
    generation_handle: &FilesystemGenerationHandle,
    target: Result<PathTarget, AccessError>,
) -> StepOutcome {
    let changes = AttributeChanges::Times(TimeChanges {
        accessed: TimeChange::Now,
        modified: TimeChange::Now,
    });
    let call = target.and_then(|target| {
        set_attributes(
            generation_handle,
            Target::Path(&target, Follow::Yes),
            changes,
        )
    });
    match call {
        Ok(call) => outcome_of(call.await),
        Err(error) => StepOutcome::Access(error),
    }
}

async fn namespace_edit(
    generation_handle: &FilesystemGenerationHandle,
    edit: Result<NamespaceEdit, AccessError>,
) -> StepOutcome {
    match edit.and_then(|edit| edit_namespace(generation_handle, edit)) {
        Ok(call) => outcome_of(call.await),
        Err(error) => StepOutcome::Access(error),
    }
}

/// Runs one step on `filesystem` through the calls that the WASI adapters use.
async fn run_step(
    agents: &UnmanagedAgents,
    filesystem: &ResidentFilesystem,
    step: &HistoryStep,
) -> StepOutcome {
    let generation_handle = resident_generation_handle(filesystem);
    let at = |path: &str| PathTarget::at_root(&generation_handle, path);
    let pair = |source: Result<PathTarget, AccessError>,
                destination: Result<PathTarget, AccessError>| {
        source.and_then(|source| destination.map(|destination| (source, destination)))
    };
    match step {
        HistoryStep::Write { file, content } => {
            write_file(
                &generation_handle,
                at(FILE_PLACES[*file]),
                CONTENTS[*content],
            )
            .await
        }
        HistoryStep::Truncate { file, size } => {
            truncate_file(&generation_handle, at(FILE_PLACES[*file]), *size).await
        }
        HistoryStep::SetTimes { file } => {
            set_times_now(&generation_handle, at(FILE_PLACES[*file])).await
        }
        HistoryStep::RemoveFile { file } => {
            let edit = at(FILE_PLACES[*file]).map(|target| NamespaceEdit::Remove {
                target,
                expected: ObjectKind::File,
            });
            namespace_edit(&generation_handle, edit).await
        }
        HistoryStep::MoveFile {
            source,
            destination,
        } => {
            let edit = pair(at(FILE_PLACES[*source]), at(FILE_PLACES[*destination])).map(
                |(source, destination)| NamespaceEdit::Move {
                    source,
                    destination,
                },
            );
            namespace_edit(&generation_handle, edit).await
        }
        HistoryStep::MoveDirectory {
            source,
            destination,
        } => {
            let edit = pair(
                at(DIRECTORY_PLACES[*source]),
                at(DIRECTORY_PLACES[*destination]),
            )
            .map(|(source, destination)| NamespaceEdit::Move {
                source,
                destination,
            });
            namespace_edit(&generation_handle, edit).await
        }
        HistoryStep::HardLink {
            source,
            destination,
        } => {
            let edit = pair(at(LINKED_PLACES[*source]), at(FILE_PLACES[*destination])).map(
                |(source, destination)| NamespaceEdit::Link {
                    source,
                    destination,
                },
            );
            namespace_edit(&generation_handle, edit).await
        }
        HistoryStep::Symlink { file, target } => {
            let edit = at(FILE_PLACES[*file]).map(|destination| NamespaceEdit::Insert {
                destination,
                object: NewObject::Symlink(SymlinkTarget(PathBuf::from(SYMLINK_TARGETS[*target]))),
            });
            namespace_edit(&generation_handle, edit).await
        }
        HistoryStep::CreateDirectory { directory } => {
            let edit =
                at(NEW_DIRECTORY_PLACES[*directory]).map(|destination| NamespaceEdit::Insert {
                    destination,
                    object: NewObject::Directory,
                });
            namespace_edit(&generation_handle, edit).await
        }
        HistoryStep::RemoveDirectory { directory } => {
            let edit = at(NEW_DIRECTORY_PLACES[*directory]).map(|target| NamespaceEdit::Remove {
                target,
                expected: ObjectKind::Directory,
            });
            namespace_edit(&generation_handle, edit).await
        }
        HistoryStep::Update { files } => {
            let files = declare_files(&agents.store, files).await;
            match update_initial_files(
                &generation_handle,
                Arc::clone(&agents.store.loader),
                agents.store.environment_id,
                files,
            ) {
                Ok(call) => outcome_of(call.await),
                Err(error) => error_outcome(error),
            }
        }
        HistoryStep::Provision { files } => {
            let files = declare_files(&agents.store, &provisioned_files(files)).await;
            match provision_initial_files(
                &generation_handle,
                Arc::clone(&agents.store.loader),
                agents.store.environment_id,
                files,
            ) {
                Ok(call) => outcome_of(call.await),
                Err(error) => error_outcome(error),
            }
        }
    }
}

async fn run_steps(
    agents: &UnmanagedAgents,
    filesystem: &ResidentFilesystem,
    steps: &[HistoryStep],
) -> Vec<StepOutcome> {
    futures::stream::iter(steps)
        .then(|step| run_step(agents, filesystem, step))
        .collect()
        .await
}

/// Reads the tree under `root` with the owner write permission bit of each object and without times.
fn tree_without_times(root: &Path) -> Tree {
    let tree = read_tree(root);
    Tree {
        nodes: tree
            .nodes
            .into_iter()
            .map(|(path, node)| {
                let node = match node {
                    Node::Directory { mode } => Node::Directory { mode: mode & 0o200 },
                    Node::File { mode, content } => Node::File {
                        mode: mode & 0o200,
                        content,
                    },
                    symlink @ Node::Symlink { .. } => symlink,
                };
                (path, node)
            })
            .collect(),
        links: tree.links,
    }
}

/// A tree with the modification times that a restore keeps.
#[derive(Debug, Eq, PartialEq)]
struct TimedTree {
    tree: Tree,
    /// The modification time of the root, at the empty path, of each directory and symlink, and of
    /// each file that a capture holds.
    modified: BTreeMap<String, std::time::SystemTime>,
}

/// Reads the tree under `root` as `tree_without_times` reads it, with the modification time of the
/// root, of each directory and symlink, and of each file that is not in `left_out`. The time of a
/// left-out file comes from the cache of the restore, so a restore does not keep it.
fn tree_with_times(root: &Path, left_out: &BTreeSet<String>) -> TimedTree {
    let root_modified = std::fs::symlink_metadata(root).unwrap().modified().unwrap();
    let modified = std::iter::once((String::new(), root_modified))
        .chain(
            list_entries(root, PathBuf::new(), Vec::new())
                .into_iter()
                .filter(|(path, metadata)| !metadata.is_file() || !left_out.contains(path))
                .map(|(path, metadata)| (path, metadata.modified().unwrap())),
        )
        .collect();
    TimedTree {
        tree: tree_without_times(root),
        modified,
    }
}

/// Gives the left-out paths of the record of `capture`.
fn left_out_of(capture: &FilesystemCapture) -> BTreeSet<String> {
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(capture.directory().join("record.json")).unwrap())
            .unwrap();
    record["left_out"]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| path.as_str().unwrap().to_string())
        .collect()
}

/// The number of histories in which the restore property compared the tree with times.
static TIMES_CHECKS: AtomicUsize = AtomicUsize::new(0);

/// The number of histories in which an install of the reference model found the old object of a
/// read-only file back at its path, with other content of the declared size.
static OLD_OBJECT_BACK_CHECKS: AtomicUsize = AtomicUsize::new(0);

/// Starts an agent from a restore of `snapshot` with the component declarations `files`.
async fn start_restored(
    agents: &UnmanagedAgents,
    name: &str,
    files: &[DeclaredFile],
    snapshot: &FilesystemCapture,
) -> (OwnedAgentId, Result<ResidentFilesystem, Error>) {
    let agent = agents.agent(name);
    let files = declare_files(&agents.store, files).await;
    let started = agents
        .start(&agent, &files, Some(copying_restore(snapshot)))
        .await;
    (agent, started)
}

/// Runs `steps` on a started agent, adds each difference from `expected` to `problems`, and
/// deletes the agent.
async fn compare_continuation(
    agents: &UnmanagedAgents,
    name: &str,
    agent: &OwnedAgentId,
    filesystem: ResidentFilesystem,
    steps: &[HistoryStep],
    expected: Expected<'_>,
    problems: &mut Vec<String>,
) {
    let outcomes = run_steps(agents, &filesystem, steps).await;
    if outcomes != expected.outcomes {
        problems.push(format!(
            "agent {name} gave {outcomes:?} after its start, and the replay gave {:?}",
            expected.outcomes
        ));
    }
    let tree = tree_without_times(&agents.root(agent));
    if &tree != expected.tree {
        problems.push(format!(
            "agent {name} holds {tree:?}, and the replay holds {:?}",
            expected.tree
        ));
    }
    if &tree != expected.model_tree {
        problems.push(format!(
            "agent {name} holds {tree:?}, and the reference model holds {:?}",
            expected.model_tree
        ));
    }
    delete(seal(filesystem)).await.unwrap();
}

/// The class of a step result. The reference model gives its results in these classes.
#[derive(Clone, Debug, Eq, PartialEq)]
enum ResultClass {
    Ok,
    NotPermitted,
    NotFound,
    AlreadyExists,
    NotEmpty,
    /// The target is not the object that the operation needs: an object above the path is not a
    /// directory, a directory is where a file must be, or a move goes into its own subtree.
    InvalidTarget,
    /// A failed install, with the first conflicting path in path order.
    Conflict(String),
    /// A result that the reference model never gives.
    Unexpected(String),
}

/// Gives the class of a lifecycle step result. This is the only function that maps step outcomes
/// to the classes of the reference model.
fn result_class(outcome: &StepOutcome) -> ResultClass {
    match outcome {
        StepOutcome::Done => ResultClass::Ok,
        StepOutcome::Access(AccessError::NotPermitted) => ResultClass::NotPermitted,
        StepOutcome::Sandbox(Some(std::io::ErrorKind::NotFound)) => ResultClass::NotFound,
        StepOutcome::Sandbox(Some(std::io::ErrorKind::AlreadyExists)) => ResultClass::AlreadyExists,
        StepOutcome::Sandbox(Some(std::io::ErrorKind::DirectoryNotEmpty)) => ResultClass::NotEmpty,
        StepOutcome::Sandbox(Some(
            std::io::ErrorKind::NotADirectory
            | std::io::ErrorKind::IsADirectory
            | std::io::ErrorKind::InvalidInput,
        )) => ResultClass::InvalidTarget,
        StepOutcome::Conflict(path) => ResultClass::Conflict(path.clone()),
        other => ResultClass::Unexpected(format!("{other:?}")),
    }
}

/// One object of the reference model.
#[derive(Clone, Debug, Eq, PartialEq)]
enum ModelObject {
    File { content: Vec<u8>, writable: bool },
    Directory,
    Symlink { target: String },
}

/// One initial-file declaration of the reference model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ModelDeclaration {
    read_only: bool,
    content: usize,
}

/// What an install of the reference model does at one path whose declaration changes.
enum ModelInstall {
    Keep,
    Seed(ModelDeclaration),
    Remove,
}

/// A reference model of an agent filesystem with initial files. It implements the initial-file
/// rule as the documentation of `plan` states it, from the tree and the declarations alone: a path
/// with equal declarations keeps what is at it, a read-only file refuses changes to its content and
/// its times, and a hard link or a rename moves the file with its permissions. It shares no code
/// with the lifecycle, and no rule of it reads an object that an install put in place.
///
/// `paths` gives the object at each path, and two paths with one object are hard links. `objects`
/// holds each object that the model made; the index of an object is its id. `component` holds the
/// component declarations and `provisioned` the entity-provisioned declarations. The declarations
/// of the filesystem are both together.
///
/// `seeded_read_only` holds the path and the object of each read-only file that an install put in
/// place. `old_object_back` tells whether an install found the old object of such a file back at
/// its path: a read-only file of the declared size with other content, at a path with a read-only
/// declaration. Both serve only the count of the histories that reach that case.
#[derive(Default)]
struct ReferenceModel {
    paths: BTreeMap<String, usize>,
    objects: Vec<ModelObject>,
    component: BTreeMap<String, ModelDeclaration>,
    provisioned: BTreeMap<String, ModelDeclaration>,
    seeded_read_only: BTreeSet<(String, usize)>,
    old_object_back: bool,
}

impl ReferenceModel {
    /// Gives the directories above `path`, from the root down.
    fn ancestors(path: &str) -> Vec<String> {
        let names = path.split('/').collect::<Vec<_>>();
        (1..names.len())
            .map(|count| names[..count].join("/"))
            .collect()
    }

    fn object_at(&self, path: &str) -> Option<&ModelObject> {
        self.paths.get(path).map(|id| &self.objects[*id])
    }

    /// Refuses a path with a missing directory above it, or with an object above it that is not a
    /// directory.
    fn check_ancestors(&self, path: &str) -> Result<(), ResultClass> {
        Self::ancestors(path)
            .iter()
            .try_for_each(|ancestor| match self.object_at(ancestor) {
                Some(ModelObject::Directory) => Ok(()),
                Some(_) => Err(ResultClass::InvalidTarget),
                None => Err(ResultClass::NotFound),
            })
    }

    /// Gives the path that `path` names after one symlink at `path`. A symlink target is relative
    /// to the directory of the symlink.
    fn follow(&self, path: &str) -> String {
        match (self.object_at(path), path.rsplit_once('/')) {
            (Some(ModelObject::Symlink { target }), Some((directory, _))) => {
                format!("{directory}/{target}")
            }
            (Some(ModelObject::Symlink { target }), None) => target.clone(),
            _ => path.to_string(),
        }
    }

    fn make(&mut self, path: &str, object: ModelObject) {
        self.objects.push(object);
        self.paths.insert(path.to_string(), self.objects.len() - 1);
    }

    fn holds_children(&self, directory: &str) -> bool {
        let prefix = format!("{directory}/");
        self.paths.keys().any(|path| path.starts_with(&prefix))
    }

    /// Tells whether `path` holds Golem's file of the old declaration `old`: a regular file whose
    /// content equals the declared content and that, where the declaration is read-only, has no
    /// write permission.
    fn holds_golem_file(&self, path: &str, old: Option<&ModelDeclaration>) -> bool {
        match (old, self.object_at(path)) {
            (Some(declared), Some(ModelObject::File { content, writable })) => {
                content[..] == *CONTENTS[declared.content] && !(declared.read_only && *writable)
            }
            _ => false,
        }
    }

    /// Tells whether `path` holds the old object of a read-only file back: a read-only file with
    /// the size of the read-only declaration `old` and other content, whose object an earlier
    /// install put at `path`. The lifecycle keeps the identity of each read-only file that it
    /// installs. It must not take that identity for Golem's file after the object came back.
    fn holds_old_object_back(&self, path: &str, old: Option<&ModelDeclaration>) -> bool {
        match (
            old,
            self.paths.get(path).map(|id| (*id, &self.objects[*id])),
        ) {
            (
                Some(declared),
                Some((
                    id,
                    ModelObject::File {
                        content,
                        writable: false,
                    },
                )),
            ) => {
                declared.read_only
                    && content.len() == CONTENTS[declared.content].len()
                    && content[..] != *CONTENTS[declared.content]
                    && self.seeded_read_only.contains(&(path.to_string(), id))
            }
            _ => false,
        }
    }

    /// Writes through a new descriptor that follows a symlink: it makes a missing file, and it
    /// replaces the content of a writable file. A read-only file refuses the write.
    fn write(&mut self, path: &str, content: usize) -> ResultClass {
        let target = self.follow(path);
        match self
            .check_ancestors(path)
            .and_then(|()| self.check_ancestors(&target))
        {
            Err(class) => class,
            Ok(()) => match self.paths.get(&target).copied() {
                None => {
                    self.make(
                        &target,
                        ModelObject::File {
                            content: CONTENTS[content].to_vec(),
                            writable: true,
                        },
                    );
                    ResultClass::Ok
                }
                Some(id) => match &mut self.objects[id] {
                    ModelObject::File {
                        writable: false, ..
                    } => ResultClass::NotPermitted,
                    ModelObject::File {
                        content: existing,
                        writable: true,
                    } => {
                        *existing = CONTENTS[content].to_vec();
                        ResultClass::Ok
                    }
                    ModelObject::Directory | ModelObject::Symlink { .. } => {
                        ResultClass::InvalidTarget
                    }
                },
            },
        }
    }

    /// Sets the size of an existing file through a new descriptor that follows a symlink. A
    /// read-only file refuses it.
    fn truncate(&mut self, path: &str, size: u64) -> ResultClass {
        let target = self.follow(path);
        match self
            .check_ancestors(path)
            .and_then(|()| self.check_ancestors(&target))
        {
            Err(class) => class,
            Ok(()) => match self.paths.get(&target).copied() {
                None => ResultClass::NotFound,
                Some(id) => match &mut self.objects[id] {
                    ModelObject::File {
                        writable: false, ..
                    } => ResultClass::NotPermitted,
                    ModelObject::File {
                        content,
                        writable: true,
                    } => {
                        content.resize(usize::try_from(size).unwrap(), 0);
                        ResultClass::Ok
                    }
                    ModelObject::Directory | ModelObject::Symlink { .. } => {
                        ResultClass::InvalidTarget
                    }
                },
            },
        }
    }

    /// Sets the times of an object through a path that follows a symlink. A read-only file refuses
    /// it, as it refuses a change to its contents.
    fn set_times(&self, path: &str) -> ResultClass {
        let target = self.follow(path);
        match self
            .check_ancestors(path)
            .and_then(|()| self.check_ancestors(&target))
        {
            Err(class) => class,
            Ok(()) => match self.object_at(&target) {
                None => ResultClass::NotFound,
                Some(ModelObject::File {
                    writable: false, ..
                }) => ResultClass::NotPermitted,
                Some(_) => ResultClass::Ok,
            },
        }
    }

    /// Removes the name of a file or a symlink. A read-only file permits it.
    fn remove_file(&mut self, path: &str) -> ResultClass {
        match self
            .check_ancestors(path)
            .map(|()| self.object_at(path).cloned())
        {
            Err(class) => class,
            Ok(None) => ResultClass::NotFound,
            Ok(Some(ModelObject::Directory)) => ResultClass::InvalidTarget,
            Ok(Some(_)) => {
                self.paths.remove(path);
                ResultClass::Ok
            }
        }
    }

    /// Renames the object at `source`, as the one rename of the lifecycle does for both kinds of
    /// object. A missing directory above either path, or an object above it that is not a
    /// directory, refuses the rename. A rename to a name of the same object changes nothing.
    ///
    /// A directory moves with all that is in it. A destination in its own subtree refuses the
    /// move, an empty directory at the destination is replaced, a directory with an object in it
    /// refuses the move, and a file or a symlink at the destination refuses it. A file or a symlink
    /// replaces another file or symlink at the destination, and a directory at the destination
    /// refuses it. A move of a read-only file, or of a directory above one, is permitted.
    fn rename(&mut self, source: &str, destination: &str) -> ResultClass {
        let inside = |path: &str, directory: &str| {
            path == directory || path.starts_with(&format!("{directory}/"))
        };
        let checked = self
            .check_ancestors(source)
            .and_then(|()| self.check_ancestors(destination));
        let moved = self.paths.get(source).copied();
        let replaced = self.paths.get(destination).copied();
        match (checked, moved, replaced) {
            (Err(class), _, _) => class,
            (Ok(()), None, _) => ResultClass::NotFound,
            (Ok(()), Some(moved), Some(replaced)) if moved == replaced => ResultClass::Ok,
            (Ok(()), Some(moved), replaced) if self.objects[moved] == ModelObject::Directory => {
                match replaced.map(|id| self.objects[id].clone()) {
                    _ if inside(destination, source) => ResultClass::InvalidTarget,
                    Some(ModelObject::Directory) if self.holds_children(destination) => {
                        ResultClass::NotEmpty
                    }
                    Some(ModelObject::File { .. } | ModelObject::Symlink { .. }) => {
                        ResultClass::InvalidTarget
                    }
                    Some(ModelObject::Directory) | None => {
                        let moved_paths = self
                            .paths
                            .iter()
                            .filter(|(path, _)| inside(path, source))
                            .map(|(path, id)| {
                                (format!("{destination}{}", &path[source.len()..]), *id)
                            })
                            .collect::<Vec<_>>();
                        self.paths.retain(|path, _| {
                            !inside(path, source) && path.as_str() != destination
                        });
                        self.paths.extend(moved_paths);
                        ResultClass::Ok
                    }
                }
            }
            (Ok(()), Some(_), Some(replaced))
                if self.objects[replaced] == ModelObject::Directory =>
            {
                ResultClass::InvalidTarget
            }
            (Ok(()), Some(moved), _) => {
                self.paths.remove(source);
                self.paths.insert(destination.to_string(), moved);
                ResultClass::Ok
            }
        }
    }

    /// Gives a file or a symlink one more name. A read-only file permits it. A directory refuses it
    /// when the source exists and the destination does not.
    fn hard_link(&mut self, source: &str, destination: &str) -> ResultClass {
        let checked = self
            .check_ancestors(source)
            .and_then(|()| self.check_ancestors(destination));
        match (
            checked,
            self.paths.get(source).copied(),
            self.paths.contains_key(destination),
        ) {
            (Err(class), _, _) => class,
            (Ok(()), None, _) => ResultClass::NotFound,
            (Ok(()), Some(_), true) => ResultClass::AlreadyExists,
            (Ok(()), Some(linked), false) if self.objects[linked] == ModelObject::Directory => {
                ResultClass::NotPermitted
            }
            (Ok(()), Some(linked), false) => {
                self.paths.insert(destination.to_string(), linked);
                ResultClass::Ok
            }
        }
    }

    fn symlink(&mut self, path: &str, target: &str) -> ResultClass {
        match (self.check_ancestors(path), self.paths.contains_key(path)) {
            (Err(class), _) => class,
            (Ok(()), true) => ResultClass::AlreadyExists,
            (Ok(()), false) => {
                self.make(
                    path,
                    ModelObject::Symlink {
                        target: target.to_string(),
                    },
                );
                ResultClass::Ok
            }
        }
    }

    fn create_directory(&mut self, path: &str) -> ResultClass {
        match (self.check_ancestors(path), self.paths.contains_key(path)) {
            (Err(class), _) => class,
            (Ok(()), true) => ResultClass::AlreadyExists,
            (Ok(()), false) => {
                self.make(path, ModelObject::Directory);
                ResultClass::Ok
            }
        }
    }

    fn remove_directory(&mut self, path: &str) -> ResultClass {
        match self
            .check_ancestors(path)
            .map(|()| self.object_at(path).cloned())
        {
            Err(class) => class,
            Ok(None) => ResultClass::NotFound,
            Ok(Some(ModelObject::Directory)) if self.holds_children(path) => ResultClass::NotEmpty,
            Ok(Some(ModelObject::Directory)) => {
                self.paths.remove(path);
                ResultClass::Ok
            }
            Ok(Some(_)) => ResultClass::InvalidTarget,
        }
    }

    /// Gives the declarations of `files` by path.
    fn declarations_of(files: &[DeclaredFile]) -> BTreeMap<String, ModelDeclaration> {
        files
            .iter()
            .map(|file| {
                (
                    file.path.to_string(),
                    ModelDeclaration {
                        read_only: file.read_only,
                        content: file.content,
                    },
                )
            })
            .collect()
    }

    /// Installs the component declarations of `files` with the entity-provisioned declarations that
    /// the filesystem has.
    fn install(&mut self, files: &[DeclaredFile]) -> ResultClass {
        let provisioned = self.provisioned.clone();
        self.install_declarations(Self::declarations_of(files), provisioned)
    }

    /// Adds the entity-provisioned declarations of `files`, and installs them with the component
    /// declarations that the filesystem has.
    fn provision(&mut self, files: &[DeclaredFile]) -> ResultClass {
        let provisioned = self
            .provisioned
            .iter()
            .map(|(path, declared)| (path.clone(), *declared))
            .chain(Self::declarations_of(files))
            .collect();
        let component = self.component.clone();
        self.install_declarations(component, provisioned)
    }

    /// Applies the initial-file rule from the current declarations to the declarations of
    /// `component` and `provisioned` together.
    ///
    /// The rule applies at each path where the two declarations differ, in path order, and an equal
    /// declaration changes nothing. At such a path the install expects what the current
    /// declarations left there: Golem's file where they declare the path, and nothing where they do
    /// not. A path that holds what the install expects gets what the new declarations give there:
    /// the new file, or nothing. A path that holds nothing, and that the new declarations do not
    /// have, stays as it is. Anything else is a conflict. A conflict fails the whole install,
    /// changes nothing, and names the first conflicting path.
    ///
    /// Each decision reads the tree as the removals of the install leave it. The install removes
    /// Golem's file where the new declarations do not have its path. So such a file above a path
    /// does not block the path. A directory at a path that the new declarations have holds nothing
    /// when every file under it is such a file and every directory in it holds at least one object.
    /// The install removes the files first, then such directories, then puts the new files in
    /// place.
    fn install_declarations(
        &mut self,
        component: BTreeMap<String, ModelDeclaration>,
        provisioned: BTreeMap<String, ModelDeclaration>,
    ) -> ResultClass {
        let together = |first: &BTreeMap<String, ModelDeclaration>,
                        second: &BTreeMap<String, ModelDeclaration>| {
            first
                .iter()
                .chain(second)
                .map(|(path, declared)| (path.clone(), *declared))
                .collect::<BTreeMap<_, _>>()
        };
        let old = together(&self.component, &self.provisioned);
        let new = together(&component, &provisioned);
        let changed = old
            .keys()
            .chain(new.keys())
            .filter(|path| old.get(*path) != new.get(*path))
            .cloned()
            .collect::<BTreeSet<String>>();
        let removed = changed
            .iter()
            .filter(|path| {
                !new.contains_key(*path)
                    && old
                        .get(*path)
                        .is_some_and(|previous| self.holds_golem_file(path, Some(previous)))
            })
            .cloned()
            .collect::<BTreeSet<String>>();
        let old_object_back = changed
            .iter()
            .any(|path| self.holds_old_object_back(path, old.get(path)));
        self.old_object_back |= old_object_back;
        let decisions = changed
            .iter()
            .map(|path| {
                let empty = self.empty_after_removals(path, new.contains_key(path), &removed);
                let expected = match old.get(path) {
                    Some(previous) => self.holds_golem_file(path, Some(previous)),
                    None => empty,
                };
                match (expected, new.get(path)) {
                    (true, Some(declared)) => Ok((path.clone(), ModelInstall::Seed(*declared))),
                    (true, None) => Ok((path.clone(), ModelInstall::Remove)),
                    (false, None) if empty => Ok((path.clone(), ModelInstall::Keep)),
                    (false, _) => Err(path.clone()),
                }
            })
            .collect::<Result<Vec<_>, String>>();
        match decisions {
            Err(path) => ResultClass::Conflict(path),
            Ok(decisions) => {
                let (removals, seeds) =
                    decisions
                        .into_iter()
                        .partition::<Vec<_>, _>(|(_, decision)| {
                            !matches!(decision, ModelInstall::Seed(_))
                        });
                removals
                    .into_iter()
                    .chain(seeds)
                    .for_each(|(path, decision)| self.install_path(&path, decision));
                self.component = component;
                self.provisioned = provisioned;
                ResultClass::Ok
            }
        }
    }

    /// Tells whether `path` holds nothing after the install removes the files at the paths
    /// `removed`. `declared` tells whether the new declarations have the path.
    ///
    /// An object above the path that is not a directory blocks the path, unless it is one of the
    /// removed files. A directory at the path blocks the path, unless the new declarations have the
    /// path, every object under the directory that is not a directory is a removed file, and every
    /// directory there holds at least one object.
    fn empty_after_removals(&self, path: &str, declared: bool, removed: &BTreeSet<String>) -> bool {
        let blocker = Self::ancestors(path).into_iter().find(|ancestor| {
            self.object_at(ancestor)
                .is_some_and(|object| *object != ModelObject::Directory)
        });
        match (blocker, self.object_at(path)) {
            (Some(blocker), _) => removed.contains(&blocker),
            (None, None) => true,
            (None, Some(ModelObject::Directory)) if declared => {
                let prefix = format!("{path}/");
                let under = self
                    .paths
                    .iter()
                    .filter(|(other, _)| other.starts_with(&prefix))
                    .map(|(other, id)| (other.as_str(), &self.objects[*id]))
                    .collect::<Vec<_>>();
                under.iter().all(|(other, object)| {
                    **object == ModelObject::Directory || removed.contains(*other)
                }) && std::iter::once(path)
                    .chain(
                        under
                            .iter()
                            .filter(|(_, object)| **object == ModelObject::Directory)
                            .map(|(other, _)| *other),
                    )
                    .all(|directory| self.holds_children(directory))
            }
            (None, Some(_)) => false,
        }
    }

    fn install_path(&mut self, path: &str, decision: ModelInstall) {
        match decision {
            ModelInstall::Keep => {}
            ModelInstall::Remove => {
                self.paths.remove(path);
            }
            ModelInstall::Seed(declared) => {
                // After the removals, a directory at the path holds only directories.
                let prefix = format!("{path}/");
                self.paths
                    .retain(|other, _| other.as_str() != path && !other.starts_with(&prefix));
                let missing = Self::ancestors(path)
                    .into_iter()
                    .filter(|ancestor| !self.paths.contains_key(ancestor))
                    .collect::<Vec<_>>();
                missing
                    .iter()
                    .for_each(|ancestor| self.make(ancestor, ModelObject::Directory));
                self.make(
                    path,
                    ModelObject::File {
                        content: CONTENTS[declared.content].to_vec(),
                        writable: !declared.read_only,
                    },
                );
                if declared.read_only {
                    self.seeded_read_only
                        .insert((path.to_string(), self.objects.len() - 1));
                }
            }
        }
    }

    /// Applies one step of a history and gives the class of its result.
    fn apply(&mut self, step: &HistoryStep) -> ResultClass {
        match step {
            HistoryStep::Write { file, content } => self.write(FILE_PLACES[*file], *content),
            HistoryStep::Truncate { file, size } => self.truncate(FILE_PLACES[*file], *size),
            HistoryStep::SetTimes { file } => self.set_times(FILE_PLACES[*file]),
            HistoryStep::RemoveFile { file } => self.remove_file(FILE_PLACES[*file]),
            HistoryStep::MoveFile {
                source,
                destination,
            } => self.rename(FILE_PLACES[*source], FILE_PLACES[*destination]),
            HistoryStep::MoveDirectory {
                source,
                destination,
            } => self.rename(DIRECTORY_PLACES[*source], DIRECTORY_PLACES[*destination]),
            HistoryStep::HardLink {
                source,
                destination,
            } => self.hard_link(LINKED_PLACES[*source], FILE_PLACES[*destination]),
            HistoryStep::Symlink { file, target } => {
                self.symlink(FILE_PLACES[*file], SYMLINK_TARGETS[*target])
            }
            HistoryStep::CreateDirectory { directory } => {
                self.create_directory(NEW_DIRECTORY_PLACES[*directory])
            }
            HistoryStep::RemoveDirectory { directory } => {
                self.remove_directory(NEW_DIRECTORY_PLACES[*directory])
            }
            HistoryStep::Update { files } => self.install(files),
            HistoryStep::Provision { files } => self.provision(&provisioned_files(files)),
        }
    }

    /// Gives the tree of the model in the form of `tree_without_times`.
    fn tree(&self) -> Tree {
        let nodes = self
            .paths
            .iter()
            .map(|(path, id)| {
                let node = match &self.objects[*id] {
                    ModelObject::Directory => Node::Directory { mode: 0o200 },
                    ModelObject::File { content, writable } => Node::File {
                        mode: if *writable { 0o200 } else { 0 },
                        content: content.clone(),
                    },
                    ModelObject::Symlink { target } => Node::Symlink {
                        target: PathBuf::from(target),
                    },
                };
                (path.clone(), node)
            })
            .collect();
        let links = self
            .paths
            .iter()
            .filter(|(_, id)| self.objects[**id] != ModelObject::Directory)
            .fold(
                BTreeMap::<usize, BTreeSet<String>>::new(),
                |mut groups, (path, id)| {
                    groups.entry(*id).or_default().insert(path.clone());
                    groups
                },
            )
            .into_values()
            .filter(|names| names.len() > 1)
            .collect();
        Tree { nodes, links }
    }
}

/// Checks one history on unmanaged storage.
///
/// Agent A replays every step. Agent B runs the steps before the capture position, and the
/// lifecycle captures it. Agent C starts from a restore of that capture with the component
/// declarations that are current at the capture, and runs the other steps. When the step after the
/// capture is an update, agent D starts from the same restore with the declarations of that update,
/// and runs the steps after it. A replay step must not make the filesystem invalid. The reference
/// model gives the result class of each replay step and the final tree, and the trees of agents A,
/// C and D must equal the tree of the model. Right after its start, the tree of agent C with the
/// modification times that a restore keeps must equal the tree of agent B at the capture.
async fn check_restore_against_replay(
    history: &History,
) -> Result<(), proptest::test_runner::TestCaseError> {
    let agents = UnmanagedAgents::new().await;
    let capture_at = history.capture;
    let (before, after) = history.steps.split_at(capture_at);
    let initial = declare_files(&agents.store, &history.initial).await;
    let mut problems = Vec::new();

    let replay_agent = agents.agent("replay");
    let replay_filesystem = agents
        .start(&replay_agent, &initial, NO_RESTORE)
        .await
        .map_err(|error| {
            proptest::test_runner::TestCaseError::fail(format!("the replay did not start: {error}"))
        })?;
    let replay_outcomes = run_steps(&agents, &replay_filesystem, &history.steps).await;
    let replay_tree = tree_without_times(&agents.root(&replay_agent));
    delete(seal(replay_filesystem)).await.unwrap();
    replay_outcomes
        .iter()
        .enumerate()
        .filter(|(_, outcome)| {
            matches!(
                outcome,
                StepOutcome::RuntimeInvalidated | StepOutcome::Access(AccessError::Revoked)
            )
        })
        .for_each(|(index, outcome)| {
            problems.push(format!("step {index} of the replay gave {outcome:?}"));
        });

    let mut model = ReferenceModel::default();
    let first_install = model.install(&history.initial);
    if first_install != ResultClass::Ok {
        problems.push(format!(
            "the reference model gave {first_install:?} for the first declarations"
        ));
    }
    let model_classes = history
        .steps
        .iter()
        .map(|step| model.apply(step))
        .collect::<Vec<_>>();
    replay_outcomes
        .iter()
        .map(result_class)
        .zip(&model_classes)
        .enumerate()
        .filter(|(_, (actual, expected))| actual != *expected)
        .for_each(|(index, (actual, expected))| {
            problems.push(format!(
                "step {index} of the replay gave {actual:?}, and the reference model gives {expected:?}"
            ));
        });
    if model.old_object_back {
        OLD_OBJECT_BACK_CHECKS.fetch_add(1, Ordering::Relaxed);
    }
    let model_tree = model.tree();
    if replay_tree != model_tree {
        problems.push(format!(
            "the replay holds {replay_tree:?}, and the reference model holds {model_tree:?}"
        ));
    }

    let captured_agent = agents.agent("captured");
    let captured = agents
        .start(&captured_agent, &initial, NO_RESTORE)
        .await
        .map_err(|error| {
            proptest::test_runner::TestCaseError::fail(format!(
                "the captured agent did not start: {error}"
            ))
        })?;
    let prefix = run_steps(&agents, &captured, before).await;
    if prefix[..] != replay_outcomes[..capture_at] {
        problems.push(format!(
            "the captured agent gave {prefix:?}, and the replay gave {:?}",
            &replay_outcomes[..capture_at]
        ));
    }
    let snapshot = capture(&captured, Duration::from_secs(5)).await;
    let at_capture = snapshot.as_ref().ok().map(|snapshot| {
        let left_out = left_out_of(snapshot);
        (
            tree_with_times(&agents.root(&captured_agent), &left_out),
            left_out,
        )
    });
    delete(seal(captured)).await.unwrap();
    match (snapshot, at_capture) {
        (Err(error), _) => problems.push(format!("the capture failed: {error}")),
        (Ok(_), None) => unreachable!("a capture that succeeded gives the tree at the capture"),
        (Ok(snapshot), Some((captured_tree, left_out))) => {
            let current = declarations_at(&history.initial, before, &prefix);
            match start_restored(&agents, "restored", &current, &snapshot).await {
                (agent, Ok(restored)) => {
                    TIMES_CHECKS.fetch_add(1, Ordering::Relaxed);
                    let restored_tree = tree_with_times(&agents.root(&agent), &left_out);
                    if restored_tree != captured_tree {
                        problems.push(format!(
                            "agent C holds {restored_tree:?} after its start, and the captured \
                             agent held {captured_tree:?} at the capture"
                        ));
                    }
                    let expected = Expected {
                        outcomes: &replay_outcomes[capture_at..],
                        tree: &replay_tree,
                        model_tree: &model_tree,
                    };
                    compare_continuation(
                        &agents,
                        "C",
                        &agent,
                        restored,
                        after,
                        expected,
                        &mut problems,
                    )
                    .await;
                }
                (_, Err(error)) => problems.push(format!("the restore did not start: {error}")),
            }
            if let Some((HistoryStep::Update { files }, rest)) = after.split_first() {
                match (
                    &replay_outcomes[capture_at],
                    start_restored(&agents, "manual", files, &snapshot).await,
                ) {
                    (StepOutcome::Done, (agent, Ok(manual))) => {
                        let expected = Expected {
                            outcomes: &replay_outcomes[capture_at + 1..],
                            tree: &replay_tree,
                            model_tree: &model_tree,
                        };
                        compare_continuation(
                            &agents,
                            "D",
                            &agent,
                            manual,
                            rest,
                            expected,
                            &mut problems,
                        )
                        .await;
                    }
                    (StepOutcome::Conflict(expected), (_, Err(error))) => {
                        let actual = error_outcome(error);
                        if actual != StepOutcome::Conflict(expected.clone()) {
                            problems.push(format!(
                                "the manual update gave {actual:?}, and the replayed update gave \
                                 the conflict {expected}"
                            ));
                        }
                    }
                    (expected, (_, Ok(manual))) => {
                        problems.push(format!(
                            "the manual update started, and the replayed update gave {expected:?}"
                        ));
                        delete(seal(manual)).await.unwrap();
                    }
                    (expected, (_, Err(error))) => problems.push(format!(
                        "the manual update failed with {error}, and the replayed update gave \
                         {expected:?}"
                    )),
                }
            }
            snapshot.discard().await.unwrap();
        }
    }
    proptest::prop_assert!(problems.is_empty(), "{}", problems.join("\n"));
    Ok(())
}

#[test]
fn a_restore_gives_the_tree_and_the_step_results_that_a_replay_gives() {
    use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|cases| cases.parse().ok())
        .unwrap_or(RESTORE_PROPERTY_CASES);
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases,
            source_file: Some(file!()),
            ..Config::default()
        },
        TestRng::from_seed(RngAlgorithm::ChaCha, &RESTORE_PROPERTY_SEED),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let started = Instant::now();

    let result = runner.run(&histories(), |history| {
        runtime.block_on(check_restore_against_replay(&history))
    });

    let elapsed = started.elapsed();
    eprintln!(
        "restore property: {cases} histories in {elapsed:?}, {:?} for each history, the tree with \
         times compared in {} histories, the old object of a read-only file back at its path in {} \
         histories",
        elapsed / cases.max(1),
        TIMES_CHECKS.load(Ordering::Relaxed),
        OLD_OBJECT_BACK_CHECKS.load(Ordering::Relaxed)
    );
    if let Err(error) = result {
        panic!("{error}");
    }
    // A run with fewer histories does not have to reach every case.
    if cases >= RESTORE_PROPERTY_CASES {
        assert!(
            TIMES_CHECKS.load(Ordering::Relaxed) > 0,
            "TIMES_CHECKS: no history compared the tree with times"
        );
        assert!(
            OLD_OBJECT_BACK_CHECKS.load(Ordering::Relaxed) > 0,
            "OLD_OBJECT_BACK_CHECKS: no history found the old object of a read-only file back at \
             its path"
        );
    }
}
