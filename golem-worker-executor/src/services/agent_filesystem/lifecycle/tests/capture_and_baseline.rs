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

/// What a tree holds: every path with its object, and the names of each object that is not a
/// directory and has several names.
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
    std::fs::hard_link(root.join("agent-link"), root.join("agent-link-name")).unwrap();
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

/// The directory places of the path space of the restore property. A directory is the only object
/// at these places.
const DIRECTORY_PLACES: [&str; 2] = ["d", "e"];

/// The file places of the path space: three names at the root and the three names in each directory.
/// Regular files and symlinks are the only objects at these places. So no step makes a hard link to a
/// directory.
const FILE_PLACES: [&str; 9] = ["f", "g", "h", "d/f", "d/g", "d/h", "e/f", "e/g", "e/h"];

/// The contents of declarations and writes. Two of them have the same size.
const CONTENTS: [&[u8]; 3] = [b"one", b"two", b"three"];

/// The targets of new symlinks. No target is outside the root. No target is a place where a symlink
/// can be, so no symlink loop occurs.
const SYMLINK_TARGETS: [&str; 4] = ["missing", "d", "d/new", "e"];

/// The number of histories that the restore property checks when `PROPTEST_CASES` is not set.
const RESTORE_PROPERTY_CASES: u32 = 2048;

/// The seed of the restore property. Each run checks the same histories.
const RESTORE_PROPERTY_SEED: [u8; 32] = *b"golem-577-restore-equals-replay!";

/// One initial file that a history declares. `place` is an index into `FILE_PLACES`.
#[derive(Clone, Debug)]
struct DeclaredFile {
    place: usize,
    read_only: bool,
    content: usize,
}

/// One step of a history: a filesystem operation of the agent, or a component update. A file place
/// is an index into `FILE_PLACES`, and a directory place is an index into `DIRECTORY_PLACES`.
#[derive(Clone, Debug)]
enum HistoryStep {
    Write { file: usize, content: usize },
    Truncate { file: usize, size: u64 },
    RemoveFile { file: usize },
    MoveFile { source: usize, destination: usize },
    MoveDirectory { source: usize, destination: usize },
    HardLink { source: usize, destination: usize },
    Symlink { file: usize, target: usize },
    CreateDirectory { directory: usize },
    RemoveDirectory { directory: usize },
    Update { files: Vec<DeclaredFile> },
}

/// A history of one agent: its first declarations, its steps, and the position of one capture.
#[derive(Clone, Debug)]
struct History {
    initial: Vec<DeclaredFile>,
    steps: Vec<HistoryStep>,
    capture: proptest::sample::Index,
}

/// What a step gave. An error keeps only the facts that do not name a host path.
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

/// The step results that another agent must give, and the final trees of the replay and of the
/// reference model.
struct Expected<'a> {
    outcomes: &'a [StepOutcome],
    tree: &'a Tree,
    model_tree: &'a Tree,
}

fn declared_files() -> impl proptest::strategy::Strategy<Value = Vec<DeclaredFile>> {
    use proptest::strategy::Strategy as _;
    proptest::collection::btree_map(
        0..FILE_PLACES.len(),
        (proptest::arbitrary::any::<bool>(), 0..CONTENTS.len()),
        0..4,
    )
    .prop_map(|files| {
        files
            .into_iter()
            .map(|(place, (read_only, content))| DeclaredFile {
                place,
                read_only,
                content,
            })
            .collect()
    })
}

fn history_step() -> impl proptest::strategy::Strategy<Value = HistoryStep> {
    use proptest::strategy::Strategy as _;
    let file = || 0..FILE_PLACES.len();
    let directory = || 0..DIRECTORY_PLACES.len();
    proptest::prop_oneof![
        3 => (file(), 0..CONTENTS.len())
            .prop_map(|(file, content)| HistoryStep::Write { file, content }),
        1 => (file(), 0_u64..3).prop_map(|(file, size)| HistoryStep::Truncate { file, size }),
        2 => file().prop_map(|file| HistoryStep::RemoveFile { file }),
        2 => (file(), file())
            .prop_map(|(source, destination)| HistoryStep::MoveFile { source, destination }),
        1 => (directory(), directory())
            .prop_map(|(source, destination)| HistoryStep::MoveDirectory { source, destination }),
        2 => (file(), file())
            .prop_map(|(source, destination)| HistoryStep::HardLink { source, destination }),
        2 => (file(), 0..SYMLINK_TARGETS.len())
            .prop_map(|(file, target)| HistoryStep::Symlink { file, target }),
        1 => directory().prop_map(|directory| HistoryStep::CreateDirectory { directory }),
        1 => directory().prop_map(|directory| HistoryStep::RemoveDirectory { directory }),
        2 => declared_files().prop_map(|files| HistoryStep::Update { files }),
    ]
}

fn histories() -> impl proptest::strategy::Strategy<Value = History> {
    use proptest::strategy::Strategy as _;
    (
        declared_files(),
        proptest::collection::vec(history_step(), 0..10),
        proptest::arbitrary::any::<proptest::sample::Index>(),
    )
        .prop_map(|(initial, steps, capture)| History {
            initial,
            steps,
            capture,
        })
}

async fn declare_files(store: &InitialFileStore, files: &[DeclaredFile]) -> Vec<InitialAgentFile> {
    futures::stream::iter(files)
        .then(|file| async move {
            let path = format!("/{}", FILE_PLACES[file.place]);
            let permissions = if file.read_only {
                AgentFilePermissions::ReadOnly
            } else {
                AgentFilePermissions::ReadWrite
            };
            store
                .declare(&path, permissions, CONTENTS[file.content])
                .await
        })
        .collect()
        .await
}

/// Gives the declarations that are current after `steps`: the files of the last update that was
/// done, or `initial`.
fn declarations_at(
    initial: &[DeclaredFile],
    steps: &[HistoryStep],
    outcomes: &[StepOutcome],
) -> Vec<DeclaredFile> {
    steps
        .iter()
        .zip(outcomes)
        .fold(initial.to_vec(), |current, step| match step {
            (HistoryStep::Update { files }, StepOutcome::Done) => files.clone(),
            _ => current,
        })
}

fn error_outcome(error: Error) -> StepOutcome {
    match error {
        Error::Access(error) => StepOutcome::Access(error),
        Error::Sandbox(error)
            if error
                .to_string()
                .contains("install a read-only initial file over other data at") =>
        {
            StepOutcome::Conflict(error.to_string())
        }
        Error::Sandbox(error) => StepOutcome::Sandbox(error.io_kind()),
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
    let file = |place: usize| PathTarget::at_root(&generation_handle, FILE_PLACES[place]);
    let directory = |place: usize| PathTarget::at_root(&generation_handle, DIRECTORY_PLACES[place]);
    let pair = |source: Result<PathTarget, AccessError>,
                destination: Result<PathTarget, AccessError>| {
        source.and_then(|source| destination.map(|destination| (source, destination)))
    };
    match step {
        HistoryStep::Write {
            file: place,
            content,
        } => write_file(&generation_handle, file(*place), CONTENTS[*content]).await,
        HistoryStep::Truncate { file: place, size } => {
            truncate_file(&generation_handle, file(*place), *size).await
        }
        HistoryStep::RemoveFile { file: place } => {
            let edit = file(*place).map(|target| NamespaceEdit::Remove {
                target,
                expected: ObjectKind::File,
            });
            namespace_edit(&generation_handle, edit).await
        }
        HistoryStep::MoveFile {
            source,
            destination,
        } => {
            let edit = pair(file(*source), file(*destination)).map(|(source, destination)| {
                NamespaceEdit::Move {
                    source,
                    destination,
                }
            });
            namespace_edit(&generation_handle, edit).await
        }
        HistoryStep::MoveDirectory {
            source,
            destination,
        } => {
            let edit =
                pair(directory(*source), directory(*destination)).map(|(source, destination)| {
                    NamespaceEdit::Move {
                        source,
                        destination,
                    }
                });
            namespace_edit(&generation_handle, edit).await
        }
        HistoryStep::HardLink {
            source,
            destination,
        } => {
            let edit = pair(file(*source), file(*destination)).map(|(source, destination)| {
                NamespaceEdit::Link {
                    source,
                    destination,
                }
            });
            namespace_edit(&generation_handle, edit).await
        }
        HistoryStep::Symlink {
            file: place,
            target,
        } => {
            let edit = file(*place).map(|destination| NamespaceEdit::Insert {
                destination,
                object: NewObject::Symlink(SymlinkTarget(PathBuf::from(SYMLINK_TARGETS[*target]))),
            });
            namespace_edit(&generation_handle, edit).await
        }
        HistoryStep::CreateDirectory { directory: place } => {
            let edit = directory(*place).map(|destination| NamespaceEdit::Insert {
                destination,
                object: NewObject::Directory,
            });
            namespace_edit(&generation_handle, edit).await
        }
        HistoryStep::RemoveDirectory { directory: place } => {
            let edit = directory(*place).map(|target| NamespaceEdit::Remove {
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
    let tree = read_tree(root, |_| false);
    Tree {
        nodes: tree
            .nodes
            .into_iter()
            .map(|(path, node)| {
                let node = match node {
                    Node::Directory { mode } => Node::Directory { mode: mode & 0o200 },
                    Node::File {
                        mode,
                        content,
                        modified,
                    } => Node::File {
                        mode: mode & 0o200,
                        content,
                        modified,
                    },
                    symlink @ Node::Symlink { .. } => symlink,
                };
                (path, node)
            })
            .collect(),
        links: tree.links,
    }
}

/// Starts an agent from a restore of `snapshot` with the declarations `files`.
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

/// The start of the error that a conflicting install gives. The conflicting path follows it.
const CONFLICT_ERROR_START: &str =
    "failed to install a read-only initial file over other data at filesystem ";

/// Gives the class of a lifecycle step result. This is the only function that maps lifecycle
/// errors to the classes of the reference model.
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
        StepOutcome::Conflict(message) => ResultClass::Conflict(
            message
                .strip_prefix(CONFLICT_ERROR_START)
                .unwrap_or(message)
                .to_string(),
        ),
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

/// A reference model of an agent filesystem with initial files. It follows the text of the GOL-577
/// issue: the initial-file rule, the read-only semantics and the sentence about equal declarations.
/// It uses no helper of the lifecycle.
///
/// `paths` gives the object at each path, and two paths with one object are hard links. `objects`
/// holds each object that the model made; the index of an object is its id. `installed` gives, for
/// each read-only declared path, the object that the last install put at the path.
#[derive(Default)]
struct ReferenceModel {
    paths: BTreeMap<String, usize>,
    objects: Vec<ModelObject>,
    declarations: BTreeMap<String, ModelDeclaration>,
    installed: BTreeMap<String, usize>,
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

    /// Tells whether an object above `path` is not a directory. An install then counts the path as
    /// holding other data, because an install never removes data that invocations made.
    fn blocked(&self, path: &str) -> bool {
        Self::ancestors(path).iter().any(|ancestor| {
            self.object_at(ancestor)
                .is_some_and(|object| *object != ModelObject::Directory)
        })
    }

    fn holds_children(&self, directory: &str) -> bool {
        let prefix = format!("{directory}/");
        self.paths.keys().any(|path| path.starts_with(&prefix))
    }

    /// Tells whether `path` holds Golem's file of the old declaration `old`, as the issue defines
    /// it: for a read-only declaration, the object that the install put there; for a read-write
    /// declaration, a file whose content equals the declared content.
    fn holds_golem_file(&self, path: &str, old: Option<&ModelDeclaration>) -> bool {
        match (old, self.paths.get(path)) {
            (Some(declared), Some(id)) if declared.read_only => {
                self.installed.get(path) == Some(id)
            }
            (Some(declared), Some(id)) => matches!(
                &self.objects[*id],
                ModelObject::File { content, .. } if content[..] == *CONTENTS[declared.content]
            ),
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

    /// Renames a file or a symlink. A name of the same object at the destination stays unchanged,
    /// and another file or symlink there is replaced. A read-only file permits the rename.
    fn move_file(&mut self, source: &str, destination: &str) -> ResultClass {
        let checked = self
            .check_ancestors(source)
            .and_then(|()| self.check_ancestors(destination));
        let moved = self.paths.get(source).copied();
        let replaced = self.paths.get(destination).copied();
        match (checked, moved, replaced) {
            (Err(class), _, _) => class,
            (Ok(()), None, _) => ResultClass::NotFound,
            (Ok(()), Some(moved), Some(replaced)) if moved == replaced => ResultClass::Ok,
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

    /// Renames the object at a directory place. A directory moves with all that is in it: an empty
    /// directory at the destination is replaced, a directory with an object in it refuses the move,
    /// and a file or a symlink at the destination refuses it. A file or a symlink at the source moves
    /// as `move_file` moves it. A move of a directory above a read-only file is permitted.
    fn move_directory(&mut self, source: &str, destination: &str) -> ResultClass {
        let inside = |path: &str, directory: &str| {
            path == directory || path.starts_with(&format!("{directory}/"))
        };
        match (
            self.object_at(source).cloned(),
            self.object_at(destination).cloned(),
        ) {
            (None, _) => ResultClass::NotFound,
            (Some(ModelObject::Directory), _) if source == destination => ResultClass::Ok,
            (Some(ModelObject::Directory), Some(ModelObject::Directory))
                if self.holds_children(destination) =>
            {
                ResultClass::NotEmpty
            }
            (
                Some(ModelObject::Directory),
                Some(ModelObject::File { .. } | ModelObject::Symlink { .. }),
            ) => ResultClass::InvalidTarget,
            (Some(ModelObject::Directory), _) => {
                let moved = self
                    .paths
                    .iter()
                    .filter(|(path, _)| inside(path, source))
                    .map(|(path, id)| (format!("{destination}{}", &path[source.len()..]), *id))
                    .collect::<Vec<_>>();
                self.paths
                    .retain(|path, _| !inside(path, source) && path.as_str() != destination);
                self.paths.extend(moved);
                ResultClass::Ok
            }
            (Some(_), _) => self.move_file(source, destination),
        }
    }

    /// Gives a file or a symlink one more name. A read-only file permits it.
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
        if self.paths.contains_key(path) {
            ResultClass::AlreadyExists
        } else {
            self.make(path, ModelObject::Directory);
            ResultClass::Ok
        }
    }

    fn remove_directory(&mut self, path: &str) -> ResultClass {
        match self.object_at(path).cloned() {
            None => ResultClass::NotFound,
            Some(ModelObject::Directory) if self.holds_children(path) => ResultClass::NotEmpty,
            Some(ModelObject::Directory) => {
                self.paths.remove(path);
                ResultClass::Ok
            }
            Some(_) => ResultClass::InvalidTarget,
        }
    }

    /// Applies the initial-file rule of the issue from the current declarations to `files`.
    ///
    /// The three parts apply at each path where the two declarations differ, in path order, and an
    /// equal declaration changes nothing. A conflict fails the whole install, changes nothing, and
    /// names the first conflicting path.
    fn install(&mut self, files: &[DeclaredFile]) -> ResultClass {
        let new = files
            .iter()
            .map(|file| {
                (
                    FILE_PLACES[file.place].to_string(),
                    ModelDeclaration {
                        read_only: file.read_only,
                        content: file.content,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let old = &self.declarations;
        let decisions = old
            .keys()
            .chain(new.keys())
            .filter(|path| old.get(*path) != new.get(*path))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|path| {
                let previous = old.get(path);
                let replaceable = !self.blocked(path)
                    && (!self.paths.contains_key(path) || self.holds_golem_file(path, previous));
                let decision = match (previous, new.get(path)) {
                    (_, Some(declared)) if declared.read_only && replaceable => {
                        ModelInstall::Seed(*declared)
                    }
                    (_, Some(declared)) if declared.read_only => return Err(path.clone()),
                    (Some(previous), Some(_)) if !previous.read_only => ModelInstall::Keep,
                    (_, Some(declared)) if replaceable => ModelInstall::Seed(*declared),
                    (Some(previous), None)
                        if previous.read_only && self.holds_golem_file(path, Some(previous)) =>
                    {
                        ModelInstall::Remove
                    }
                    _ => ModelInstall::Keep,
                };
                Ok((path.clone(), decision))
            })
            .collect::<Result<Vec<_>, String>>();
        match decisions {
            Err(path) => ResultClass::Conflict(path),
            Ok(decisions) => {
                decisions
                    .into_iter()
                    .for_each(|(path, decision)| self.install_path(&path, decision));
                self.declarations = new;
                ResultClass::Ok
            }
        }
    }

    fn install_path(&mut self, path: &str, decision: ModelInstall) {
        match decision {
            ModelInstall::Keep => {
                self.installed.remove(path);
            }
            ModelInstall::Remove => {
                self.paths.remove(path);
                self.installed.remove(path);
            }
            ModelInstall::Seed(declared) => {
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
                    let object = self.paths[path];
                    self.installed.insert(path.to_string(), object);
                } else {
                    self.installed.remove(path);
                }
            }
        }
    }

    /// Applies one step of a history and gives the class of its result.
    fn apply(&mut self, step: &HistoryStep) -> ResultClass {
        match step {
            HistoryStep::Write { file, content } => self.write(FILE_PLACES[*file], *content),
            HistoryStep::Truncate { file, size } => self.truncate(FILE_PLACES[*file], *size),
            HistoryStep::RemoveFile { file } => self.remove_file(FILE_PLACES[*file]),
            HistoryStep::MoveFile {
                source,
                destination,
            } => self.move_file(FILE_PLACES[*source], FILE_PLACES[*destination]),
            HistoryStep::MoveDirectory {
                source,
                destination,
            } => self.move_directory(DIRECTORY_PLACES[*source], DIRECTORY_PLACES[*destination]),
            HistoryStep::HardLink {
                source,
                destination,
            } => self.hard_link(FILE_PLACES[*source], FILE_PLACES[*destination]),
            HistoryStep::Symlink { file, target } => {
                self.symlink(FILE_PLACES[*file], SYMLINK_TARGETS[*target])
            }
            HistoryStep::CreateDirectory { directory } => {
                self.create_directory(DIRECTORY_PLACES[*directory])
            }
            HistoryStep::RemoveDirectory { directory } => {
                self.remove_directory(DIRECTORY_PLACES[*directory])
            }
            HistoryStep::Update { files } => self.install(files),
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
                        modified: None,
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
/// lifecycle captures it. Agent C starts from a restore of that capture with the declarations that
/// are current at the capture, and runs the other steps. When the step after the capture is an
/// update, agent D starts from the same restore with the declarations of that update, and runs the
/// steps after it. A replay step must not make the filesystem invalid. The reference model gives the
/// result class of each replay step and the final tree, and the trees of agents A, C and D must
/// equal the tree of the model.
async fn check_restore_against_replay(
    history: &History,
) -> Result<(), proptest::test_runner::TestCaseError> {
    let agents = UnmanagedAgents::new().await;
    let capture_at = history.capture.index(history.steps.len() + 1);
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
    delete(seal(captured)).await.unwrap();
    match snapshot {
        Err(error) => problems.push(format!("the capture failed: {error}")),
        Ok(snapshot) => {
            let current = declarations_at(&history.initial, before, &prefix);
            match start_restored(&agents, "restored", &current, &snapshot).await {
                (agent, Ok(restored)) => {
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
        "restore property: {cases} histories in {elapsed:?}, {:?} for each history",
        elapsed / cases.max(1)
    );
    if let Err(error) = result {
        panic!("{error}");
    }
}
