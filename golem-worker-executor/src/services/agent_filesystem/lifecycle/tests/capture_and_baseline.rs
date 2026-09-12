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
use std::time::SystemTime;
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

fn file_attributes(
    object: u64,
    created: Option<SystemTime>,
    link_count: u64,
    read_only: bool,
) -> SandboxAttributes {
    SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count,
        size: 0,
        accessed: None,
        modified: None,
        created,
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

fn record(
    initial: &[&InitialAgentFile],
    read_only: serde_json::Value,
    link_groups: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "initial_files": initial,
        "provisioned_files": [],
        "read_only_files": read_only,
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
async fn capture_records_golem_read_only_files_and_leaves_out_those_with_a_single_name() {
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
        read_only("/linked").await,
        read_only("/no-time").await,
        read_only("/no-time-writable").await,
        read_only("/renamed").await,
        read_only("/replaced").await,
        read_only("/reused").await,
        read_only("/single").await,
        store
            .declare("/writable", AgentFilePermissions::ReadWrite, b"writable")
            .await,
    ];
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let later = time + Duration::from_secs(1);
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    files.iter().for_each(|_| control.push_seed(Ok(())));
    [
        file_attributes(2, Some(time), 1, true),
        file_attributes(7, None, 1, true),
        file_attributes(8, None, 1, true),
        file_attributes(3, Some(time), 1, true),
        file_attributes(4, Some(time), 1, true),
        file_attributes(6, Some(time), 1, true),
        file_attributes(1, Some(time), 1, true),
    ]
    .into_iter()
    .for_each(|attributes| control.push_get_attributes(Ok(attributes)));
    let prepared = store.prepare(&files).await;
    let resident = scripted_resident(&control, filesystem, prepared).await;
    [
        Ok(file_attributes(2, Some(time), 2, true)),
        Ok(file_attributes(7, None, 1, true)),
        Ok(file_attributes(8, None, 1, false)),
        Err(missing("read renamed initial file")),
        Ok(file_attributes(5, Some(time), 1, false)),
        Ok(file_attributes(6, Some(later), 1, true)),
        Ok(file_attributes(1, Some(time), 1, true)),
    ]
    .into_iter()
    .for_each(|attributes| control.push_get_attributes(attributes));
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
        copy_call.contains(r#"excluded=["no-time", "single"]"#),
        "{copy_call}"
    );
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(captured.directory().join("record.json")).unwrap())
            .unwrap();
    let recorded = |path: &str, left_out: bool| {
        let file = files
            .iter()
            .find(|file| file.path.to_rel_string() == path)
            .unwrap();
        serde_json::json!({
            "path": path,
            "content_hash": file.content_hash,
            "left_out": left_out,
        })
    };
    assert_eq!(
        record["read_only_files"],
        serde_json::json!([
            recorded("linked", false),
            recorded("no-time", true),
            recorded("single", true),
        ])
    );
    assert_eq!(
        record["link_groups"],
        serde_json::json!([{ "first": "linked", "others": ["elsewhere"] }])
    );
    assert_eq!(record["initial_files"], serde_json::json!(files));
    assert_eq!(record["provisioned_files"], serde_json::json!([]));
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
        serde_json::json!([{ "path": "kept", "content_hash": kept.content_hash, "left_out": true }]),
        serde_json::json!([{ "first": "data/a", "others": ["data/b"] }]),
    );
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    control.push_seed(Ok(()));
    control.push_hard_link(Ok(()));
    control.push_seed(Ok(()));
    control.push_get_attributes(Ok(file_attributes(1, None, 1, true)));
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
        ["seed", "hard_link", "seed", "get_path_attributes"]
    );
    assert!(
        calls[0].contains(&format!(
            "source={}, target=SandboxPath {{ base: Root, path: \"\" }}, access=FromSource, existing=Fail",
            into.join("tree").display()
        )),
        "{}",
        calls[0]
    );
    assert!(
        calls[2].contains(r#"path: "kept" }, access=ReadOnly, existing=Fail"#),
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
async fn a_failed_discard_of_the_restore_directory_keeps_the_baseline_successful() {
    let store = InitialFileStore::new().await;
    let capture_record = record(&[], serde_json::json!([]), serde_json::json!([]));
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    control.push_seed(Ok(()));
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
        created: None,
        read_only: false,
        object: SandboxObjectId::scripted(5),
    };
    let reads_before = call_count(&control, "get_path_attributes(");
    control.push_get_attributes(Ok(object(SandboxObjectKind::File, 0)));
    control.push_get_attributes(Ok(object(SandboxObjectKind::Directory, old_directory.size)));
    control.push_get_attributes(Ok(object(SandboxObjectKind::File, old_same_size.size)));

    let updated = update_initial_files(
        &resident_generation_handle(&resident),
        Arc::clone(&store.loader),
        store.environment_id,
        vec![new_directory, new_same_size, below_a_file],
    )
    .unwrap()
    .await;

    assert!(
        updated.is_ok(),
        "an update that keeps every path must read only the paths and the first object above a \
         path that is not a directory: {updated:?}"
    );
    assert_eq!(
        call_count(&control, "get_path_attributes(") - reads_before,
        3
    );
    assert!(!has_call(&control, "open("));
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
    control.push_get_attributes(Ok(file_attributes(1, None, 1, true)));
    let resident = scripted_resident(
        &control,
        filesystem,
        store.prepare(std::slice::from_ref(&old)).await,
    )
    .await;
    let generation_handle = resident_generation_handle(&resident);
    control.push_get_attributes(Ok(file_attributes(9, None, 1, false)));

    let conflict = update_initial_files(
        &generation_handle,
        Arc::clone(&store.loader),
        store.environment_id,
        vec![old.clone(), added],
    )
    .unwrap()
    .await
    .unwrap_err();

    assert!(conflict.to_string().contains("added"), "{conflict}");
    assert_eq!(call_count(&control, "seed("), 1);
    assert!(!filesystem_activity(&resident).has_terminal_failure());

    control.push_get_attributes(Ok(file_attributes(1, None, 1, true)));
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

/// Copies what is under `from` into `into` with permissions and file modification times.
fn copy_directory_contents(from: &Path, into: &Path) {
    std::fs::read_dir(from)
        .unwrap()
        .map(|entry| entry.unwrap())
        .for_each(|entry| {
            let source = entry.path();
            let target = into.join(entry.file_name());
            let metadata = std::fs::symlink_metadata(&source).unwrap();
            if metadata.file_type().is_symlink() {
                std::os::unix::fs::symlink(std::fs::read_link(&source).unwrap(), &target).unwrap();
            } else if metadata.is_dir() {
                std::fs::create_dir(&target).unwrap();
                copy_directory_contents(&source, &target);
                std::fs::set_permissions(&target, metadata.permissions()).unwrap();
            } else {
                std::fs::copy(&source, &target).unwrap();
                std::fs::File::open(&target)
                    .unwrap()
                    .set_modified(metadata.modified().unwrap())
                    .unwrap();
            }
        });
}

/// What a tree holds at one path.
#[derive(Debug, Eq, PartialEq)]
enum Node {
    Directory {
        mode: u32,
    },
    File {
        mode: u32,
        content: Vec<u8>,
        modified: Option<SystemTime>,
    },
    Symlink {
        target: PathBuf,
    },
}

/// What a tree holds: every path with its object, and the names of each file with several names.
#[derive(Debug, Eq, PartialEq)]
struct Tree {
    nodes: BTreeMap<String, Node>,
    links: BTreeSet<BTreeSet<String>>,
}

/// Reads the tree under `root`. The modification time of a regular file is part of the result only
/// where `with_modified` holds for its path.
fn read_tree(root: &Path, with_modified: impl Fn(&str) -> bool) -> Tree {
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
                    modified: with_modified(path).then(|| metadata.modified().unwrap()),
                }
            };
            (path.clone(), node)
        })
        .collect();
    let links = entries
        .iter()
        .filter(|(_, metadata)| metadata.is_file() && metadata.nlink() > 1)
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
async fn capture_then_restore_on_unmanaged_storage_gives_back_the_same_tree() {
    let agents = UnmanagedAgents::new().await;
    let store = &agents.store;
    let files = [
        store
            .declare("/ro-kept.txt", AgentFilePermissions::ReadOnly, b"kept")
            .await,
        store
            .declare(
                "/ro-deleted.txt",
                AgentFilePermissions::ReadOnly,
                b"deleted",
            )
            .await,
        store
            .declare(
                "/ro-renamed.txt",
                AgentFilePermissions::ReadOnly,
                b"renamed",
            )
            .await,
        store
            .declare("/ro-linked.txt", AgentFilePermissions::ReadOnly, b"linked")
            .await,
        store
            .declare(
                "/rw-modified.txt",
                AgentFilePermissions::ReadWrite,
                b"modified",
            )
            .await,
        store
            .declare(
                "/rw-deleted.txt",
                AgentFilePermissions::ReadWrite,
                b"deleted",
            )
            .await,
        store
            .declare(
                "/dir/ro-in-dir.txt",
                AgentFilePermissions::ReadOnly,
                b"in dir",
            )
            .await,
        store
            .declare(
                "/nested/ro-nested.txt",
                AgentFilePermissions::ReadOnly,
                b"nested",
            )
            .await,
    ];
    let captured_agent = agents.agent("captured");
    let resident = agents
        .start(&captured_agent, &files, NO_RESTORE)
        .await
        .unwrap();
    let provisioned = store
        .declare(
            "/ro-provisioned.txt",
            AgentFilePermissions::ReadOnly,
            b"provisioned",
        )
        .await;
    provision_initial_files(
        &resident_generation_handle(&resident),
        Arc::clone(&store.loader),
        store.environment_id,
        vec![provisioned.clone()],
    )
    .unwrap()
    .await
    .unwrap();
    let root = agents.root(&captured_agent);
    std::fs::remove_file(root.join("ro-deleted.txt")).unwrap();
    std::fs::write(root.join("rw-modified.txt"), b"modified by the agent").unwrap();
    std::fs::remove_file(root.join("rw-deleted.txt")).unwrap();
    std::fs::rename(root.join("ro-renamed.txt"), root.join("renamed.txt")).unwrap();
    std::fs::hard_link(root.join("ro-linked.txt"), root.join("second-name.txt")).unwrap();
    std::fs::rename(root.join("dir"), root.join("moved-dir")).unwrap();
    std::fs::write(root.join("agent.txt"), b"agent data").unwrap();
    std::fs::hard_link(root.join("agent.txt"), root.join("agent-alias.txt")).unwrap();
    std::os::unix::fs::symlink("agent.txt", root.join("agent-link")).unwrap();
    let written = |path: &str| matches!(path, "rw-modified.txt" | "agent.txt" | "agent-alias.txt");
    let expected = read_tree(&root, written);

    let captured = capture(&resident, Duration::from_secs(5)).await.unwrap();

    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(captured.directory().join("record.json")).unwrap())
            .unwrap();
    let recorded = record["read_only_files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap().to_string(),
                file["left_out"].as_bool().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recorded,
        [
            ("nested/ro-nested.txt".to_string(), true),
            ("ro-kept.txt".to_string(), true),
            ("ro-linked.txt".to_string(), false),
            ("ro-provisioned.txt".to_string(), true),
        ]
    );
    assert_eq!(
        record["provisioned_files"],
        serde_json::json!([provisioned])
    );
    let restored_agent = agents.agent("restored");
    let restored = agents
        .start(&restored_agent, &files, Some(copying_restore(&captured)))
        .await
        .unwrap();
    assert_eq!(read_tree(&agents.root(&restored_agent), written), expected);
    let captured_again = capture(&restored, Duration::from_secs(5)).await.unwrap();
    let record_again: serde_json::Value = serde_json::from_slice(
        &std::fs::read(captured_again.directory().join("record.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        record_again["read_only_files"], record["read_only_files"],
        "the read-only files that a restore puts in place must stay Golem's files"
    );
    captured_again.discard().await.unwrap();
    captured.discard().await.unwrap();
    assert!(scratch_is_empty(&agents.scratch));
    delete(seal(resident)).await.unwrap();
    delete(seal(restored)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn automatic_and_manual_updates_give_the_same_files_for_every_branch_of_the_rule() {
    let agents = UnmanagedAgents::new().await;
    let store = &agents.store;
    let ro = AgentFilePermissions::ReadOnly;
    let rw = AgentFilePermissions::ReadWrite;
    let old = [
        store.declare("/ro-unchanged.txt", ro, b"unchanged").await,
        store
            .declare("/ro-unchanged-deleted.txt", ro, b"unchanged")
            .await,
        store.declare("/ro-changed.txt", ro, b"old").await,
        store.declare("/ro-changed-deleted.txt", ro, b"old").await,
        store.declare("/ro-dropped.txt", ro, b"dropped").await,
        store
            .declare("/ro-dropped-replaced.txt", ro, b"dropped")
            .await,
        store
            .declare("/rw-unchanged-modified.txt", rw, b"unchanged")
            .await,
        store.declare("/rw-changed-modified.txt", rw, b"old").await,
        store.declare("/rw-to-ro.txt", rw, b"same").await,
        store.declare("/ro-to-rw.txt", ro, b"same").await,
        store.declare("/ro-to-rw-replaced.txt", ro, b"same").await,
        store.declare("/rw-dropped.txt", rw, b"dropped").await,
    ];
    let new = [
        old[0].clone(),
        old[1].clone(),
        store.declare("/ro-changed.txt", ro, b"new").await,
        store.declare("/ro-changed-deleted.txt", ro, b"new").await,
        old[6].clone(),
        store.declare("/rw-changed-modified.txt", rw, b"new").await,
        store.declare("/rw-to-ro.txt", ro, b"same").await,
        store.declare("/ro-to-rw.txt", rw, b"same").await,
        store.declare("/ro-to-rw-replaced.txt", rw, b"same").await,
        store.declare("/rw-added.txt", rw, b"added").await,
        store.declare("/rw-added-existing.txt", rw, b"added").await,
        store.declare("/ro-added.txt", ro, b"added").await,
    ];
    let invocations = |root: &Path| {
        std::fs::remove_file(root.join("ro-unchanged-deleted.txt")).unwrap();
        std::fs::remove_file(root.join("ro-changed-deleted.txt")).unwrap();
        std::fs::remove_file(root.join("ro-dropped-replaced.txt")).unwrap();
        std::fs::write(root.join("ro-dropped-replaced.txt"), b"agent").unwrap();
        std::fs::write(root.join("rw-unchanged-modified.txt"), b"agent").unwrap();
        std::fs::write(root.join("rw-changed-modified.txt"), b"agent").unwrap();
        std::fs::remove_file(root.join("ro-to-rw-replaced.txt")).unwrap();
        std::fs::write(root.join("ro-to-rw-replaced.txt"), b"agent").unwrap();
        std::fs::write(root.join("rw-added-existing.txt"), b"agent").unwrap();
    };

    let automatic_agent = agents.agent("automatic");
    let automatic = agents
        .start(&automatic_agent, &old, NO_RESTORE)
        .await
        .unwrap();
    invocations(&agents.root(&automatic_agent));
    update_initial_files(
        &resident_generation_handle(&automatic),
        Arc::clone(&store.loader),
        store.environment_id,
        new.to_vec(),
    )
    .unwrap()
    .await
    .unwrap();
    let automatic_tree = read_tree(&agents.root(&automatic_agent), |_| false);

    let source_agent = agents.agent("manual-source");
    let source = agents.start(&source_agent, &old, NO_RESTORE).await.unwrap();
    invocations(&agents.root(&source_agent));
    let captured = capture(&source, Duration::from_secs(5)).await.unwrap();
    let manual_agent = agents.agent("manual");
    let manual = agents
        .start(&manual_agent, &new, Some(copying_restore(&captured)))
        .await
        .unwrap();
    let manual_tree = read_tree(&agents.root(&manual_agent), |_| false);

    assert_eq!(manual_tree, automatic_tree);
    let file = |path: &str| match automatic_tree.nodes.get(path) {
        Some(Node::File { mode, content, .. }) => Some((mode & 0o222 == 0, content.as_slice())),
        _ => None,
    };
    assert_eq!(
        file("ro-unchanged.txt"),
        Some((true, b"unchanged".as_slice()))
    );
    assert_eq!(file("ro-unchanged-deleted.txt"), None);
    assert_eq!(file("ro-changed.txt"), Some((true, b"new".as_slice())));
    assert_eq!(
        file("ro-changed-deleted.txt"),
        Some((true, b"new".as_slice()))
    );
    assert_eq!(file("ro-dropped.txt"), None);
    assert_eq!(
        file("ro-dropped-replaced.txt"),
        Some((false, b"agent".as_slice()))
    );
    assert_eq!(
        file("rw-unchanged-modified.txt"),
        Some((false, b"agent".as_slice()))
    );
    assert_eq!(
        file("rw-changed-modified.txt"),
        Some((false, b"agent".as_slice()))
    );
    assert_eq!(file("rw-to-ro.txt"), Some((true, b"same".as_slice())));
    assert_eq!(file("ro-to-rw.txt"), Some((false, b"same".as_slice())));
    assert_eq!(
        file("ro-to-rw-replaced.txt"),
        Some((false, b"agent".as_slice()))
    );
    assert_eq!(file("rw-dropped.txt"), Some((false, b"dropped".as_slice())));
    assert_eq!(file("rw-added.txt"), Some((false, b"added".as_slice())));
    assert_eq!(
        file("rw-added-existing.txt"),
        Some((false, b"agent".as_slice()))
    );
    assert_eq!(file("ro-added.txt"), Some((true, b"added".as_slice())));
    captured.discard().await.unwrap();
    delete(seal(automatic)).await.unwrap();
    delete(seal(source)).await.unwrap();
    delete(seal(manual)).await.unwrap();
}

#[test]
#[timeout("120s")]
async fn each_conflict_fails_the_install_with_its_path_and_changes_nothing() {
    let agents = UnmanagedAgents::new().await;
    let store = &agents.store;
    let ro = AgentFilePermissions::ReadOnly;
    let rw = AgentFilePermissions::ReadWrite;
    let target = store.declare("/target.txt", ro, b"initial").await;
    let source = store.declare("/source.txt", ro, b"initial").await;
    let writable_target = store.declare("/target.txt", rw, b"initial").await;
    // A name, the old declarations, the new declarations, and what an invocation does.
    type ConflictCase = (
        &'static str,
        Vec<InitialAgentFile>,
        Vec<InitialAgentFile>,
        fn(&Path),
    );
    let cases: [ConflictCase; 4] = [
        (
            "a file that an invocation wrote",
            vec![],
            vec![target.clone()],
            |root| std::fs::write(root.join("target.txt"), b"agent").unwrap(),
        ),
        (
            "a changed read-write file",
            vec![writable_target.clone()],
            vec![target.clone()],
            |root| std::fs::write(root.join("target.txt"), b"changed").unwrap(),
        ),
        (
            "a read-only file that the agent moved there",
            vec![source.clone()],
            vec![source.clone(), target.clone()],
            |root| std::fs::rename(root.join("source.txt"), root.join("target.txt")).unwrap(),
        ),
        ("a directory", vec![], vec![target.clone()], |root| {
            std::fs::create_dir(root.join("target.txt")).unwrap()
        }),
    ];

    futures::stream::iter(cases.into_iter().enumerate())
        .for_each(|(index, (name, old, new, invocation))| {
            let agents = &agents;
            async move {
                let automatic_agent = agents.agent(&format!("automatic-{index}"));
                let automatic = agents
                    .start(&automatic_agent, &old, NO_RESTORE)
                    .await
                    .unwrap();
                let root = agents.root(&automatic_agent);
                invocation(&root);
                let before = read_tree(&root, |_| true);
                let error = update_initial_files(
                    &resident_generation_handle(&automatic),
                    Arc::clone(&agents.store.loader),
                    agents.store.environment_id,
                    new.clone(),
                )
                .unwrap()
                .await
                .unwrap_err();
                assert!(error.to_string().contains("target.txt"), "{name}: {error}");
                assert_eq!(read_tree(&root, |_| true), before, "{name}");
                assert!(
                    !filesystem_activity(&automatic).has_terminal_failure(),
                    "{name}"
                );

                let source_agent = agents.agent(&format!("manual-source-{index}"));
                let source = agents.start(&source_agent, &old, NO_RESTORE).await.unwrap();
                invocation(&agents.root(&source_agent));
                let captured = capture(&source, Duration::from_secs(5)).await.unwrap();
                let error = agents
                    .start(
                        &agents.agent(&format!("manual-{index}")),
                        &new,
                        Some(copying_restore(&captured)),
                    )
                    .await
                    .err()
                    .unwrap();
                assert!(error.to_string().contains("target.txt"), "{name}: {error}");
                captured.discard().await.unwrap();
                delete(seal(automatic)).await.unwrap();
                delete(seal(source)).await.unwrap();
            }
        })
        .await;
}

#[test]
#[timeout("60s")]
async fn read_only_initial_files_follow_their_permission_bits_after_rename_link_and_directory_move()
{
    let agents = UnmanagedAgents::new().await;
    let store = &agents.store;
    let files = [
        store
            .declare("/ro.txt", AgentFilePermissions::ReadOnly, b"read only")
            .await,
        store
            .declare(
                "/dir/ro-in-dir.txt",
                AgentFilePermissions::ReadOnly,
                b"in dir",
            )
            .await,
        store
            .declare(
                "/ro-deleted.txt",
                AgentFilePermissions::ReadOnly,
                b"deleted",
            )
            .await,
        store
            .declare("/rw.txt", AgentFilePermissions::ReadWrite, b"read write")
            .await,
    ];
    let agent = agents.agent("permission-bits");
    let resident = agents.start(&agent, &files, NO_RESTORE).await.unwrap();
    let generation_handle = resident_generation_handle(&resident);
    let at = |path: &str| PathTarget::at_root(&generation_handle, path).unwrap();
    let refused = |result: Result<Opened, Error>| async move {
        match result {
            Ok(opened) => {
                close(opened.node).await.unwrap();
                false
            }
            Err(error) => matches!(error, Error::Access(AccessError::NotPermitted)),
        }
    };
    let refusals = |path: &'static str| {
        let generation_handle = &generation_handle;
        async move {
            let Ok(target) = PathTarget::at_root(generation_handle, path) else {
                return [false; 5];
            };
            let write = match open(
                generation_handle,
                target.clone(),
                OpenOptions::Existing {
                    expected: ObjectKind::File,
                    access: AccessMode::Write,
                    follow: Follow::Yes,
                },
            ) {
                Ok(call) => refused(call.await).await,
                Err(_) => false,
            };
            let truncate = match open(
                generation_handle,
                target.clone(),
                OpenOptions::File {
                    access: AccessMode::Write,
                    disposition: FileDisposition::TruncateExisting,
                    follow: Follow::Yes,
                },
            ) {
                Ok(call) => refused(call.await).await,
                Err(_) => false,
            };
            let times = match set_attributes(
                generation_handle,
                Target::Path(&target, Follow::Yes),
                AttributeChanges::Times(TimeChanges {
                    accessed: TimeChange::Now,
                    modified: TimeChange::Now,
                }),
            ) {
                Ok(call) => matches!(call.await, Err(Error::Access(AccessError::NotPermitted))),
                Err(_) => false,
            };
            let reader = match open(
                generation_handle,
                target,
                OpenOptions::Existing {
                    expected: ObjectKind::File,
                    access: AccessMode::Read,
                    follow: Follow::Yes,
                },
            ) {
                Ok(call) => call.await.ok(),
                Err(_) => None,
            };
            let refuses_descriptor_change = |changes: AttributeChanges| {
                reader.as_ref().is_some_and(|reader| {
                    set_attributes(generation_handle, Target::Open(&reader.node), changes).map(drop)
                        == Err(AccessError::NotPermitted)
                })
            };
            let descriptor_size = refuses_descriptor_change(AttributeChanges::File {
                size: 0,
                times: TimeChanges {
                    accessed: TimeChange::Keep,
                    modified: TimeChange::Keep,
                },
            });
            let descriptor_times =
                refuses_descriptor_change(AttributeChanges::Times(TimeChanges {
                    accessed: TimeChange::Now,
                    modified: TimeChange::Now,
                }));
            if let Some(reader) = reader {
                close(reader.node).await.unwrap();
            }
            [write, truncate, times, descriptor_size, descriptor_times]
        }
    };
    let edit = |edit: NamespaceEdit| {
        let generation_handle = &generation_handle;
        async move {
            edit_namespace(generation_handle, edit)
                .unwrap()
                .await
                .unwrap()
        }
    };

    assert_eq!(refusals("ro.txt").await, [true; 5], "the installed file");
    edit(NamespaceEdit::Remove {
        target: at("ro-deleted.txt"),
        expected: ObjectKind::File,
    })
    .await;
    edit(NamespaceEdit::Move {
        source: at("ro.txt"),
        destination: at("renamed.txt"),
    })
    .await;
    edit(NamespaceEdit::Link {
        source: at("renamed.txt"),
        destination: at("linked.txt"),
    })
    .await;
    edit(NamespaceEdit::Move {
        source: at("dir"),
        destination: at("moved-dir"),
    })
    .await;
    let writer = open(
        &generation_handle,
        at("rw.txt"),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Write,
            follow: Follow::Yes,
        },
    )
    .unwrap()
    .await
    .unwrap();
    let OpenNode::File(writable) = &writer.node else {
        panic!("rw.txt must open as a file")
    };
    write(
        &generation_handle,
        writable,
        WritePlacement::At(0),
        Bytes::from_static(b"written"),
    )
    .unwrap()
    .await
    .unwrap();
    close(writer.node).await.unwrap();
    let created = open(
        &generation_handle,
        at("moved-dir/new.txt"),
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
    assert_eq!(refusals("renamed.txt").await, [true; 5], "after a rename");
    assert_eq!(refusals("linked.txt").await, [true; 5], "after a hard link");
    assert_eq!(
        refusals("moved-dir/ro-in-dir.txt").await,
        [true; 5],
        "after a move of the directory above it"
    );

    let root = agents.root(&agent);
    assert_eq!(std::fs::read(root.join("rw.txt")).unwrap(), b"writtenite");
    assert!(root.join("moved-dir/new.txt").is_file());
    assert!(!root.join("ro-deleted.txt").exists());
    delete(seal(resident)).await.unwrap();
}
