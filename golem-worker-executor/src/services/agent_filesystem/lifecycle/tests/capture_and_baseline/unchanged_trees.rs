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

//! The counters of a generation, and what a capture gives against a mark: an unchanged tree, a
//! tree of initial files, or a copy with its change detection.

use super::*;
use test_r::{test, timeout};

const WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    Unchanged,
    InitialFiles,
    SizeMtime,
    Full,
}

/// Captures against `since`, discards the copy, and gives what the capture saw and the mark to
/// compare with next. A capture that copies nothing gives `since` back.
async fn look(filesystem: &ResidentFilesystem, since: TreeMark) -> (Seen, TreeMark) {
    match capture(filesystem, WAIT, Some(since)).await.unwrap() {
        CaptureOutcome::Unchanged => (Seen::Unchanged, since),
        CaptureOutcome::InitialFiles => (Seen::InitialFiles, since),
        CaptureOutcome::Captured {
            capture,
            mark,
            detection,
        } => {
            capture.discard().await.unwrap();
            let seen = match detection {
                ChangeDetection::SizeMtime => Seen::SizeMtime,
                ChangeDetection::Full => Seen::Full,
            };
            (seen, mark)
        }
    }
}

/// Captures without a mark and gives what the capture saw.
async fn look_without_mark(filesystem: &ResidentFilesystem) -> Seen {
    match capture(filesystem, WAIT, None).await.unwrap() {
        CaptureOutcome::Unchanged => Seen::Unchanged,
        CaptureOutcome::InitialFiles => Seen::InitialFiles,
        CaptureOutcome::Captured {
            capture, detection, ..
        } => {
            capture.discard().await.unwrap();
            match detection {
                ChangeDetection::SizeMtime => Seen::SizeMtime,
                ChangeDetection::Full => Seen::Full,
            }
        }
    }
}

/// Captures without a mark, as the first upload of an agent does, and gives the mark of the copy.
async fn first_mark(filesystem: &ResidentFilesystem) -> TreeMark {
    match capture(filesystem, WAIT, None).await.unwrap() {
        CaptureOutcome::Captured {
            capture,
            mark,
            detection,
        } => {
            assert_eq!(detection, ChangeDetection::Full);
            capture.discard().await.unwrap();
            mark
        }
        CaptureOutcome::Unchanged | CaptureOutcome::InitialFiles => {
            panic!("a capture without a mark of a changed tree copied nothing")
        }
    }
}

/// Captures a changed tree and gives the copy.
async fn copy_of(filesystem: &ResidentFilesystem) -> FilesystemCapture {
    match capture(filesystem, WAIT, None).await.unwrap() {
        CaptureOutcome::Captured { capture, .. } => capture,
        CaptureOutcome::Unchanged | CaptureOutcome::InitialFiles => {
            panic!("a capture of a changed tree copied nothing")
        }
    }
}

#[derive(Clone, Copy)]
struct Times {
    accessed: std::time::SystemTime,
    modified: std::time::SystemTime,
}

async fn times_of(generation_handle: &FilesystemGenerationHandle, path: &str) -> Times {
    let target = PathTarget::at_root(generation_handle, path).unwrap();
    let attributes = attributes(generation_handle, Target::Path(&target, Follow::No))
        .unwrap()
        .await
        .unwrap();
    Times {
        accessed: attributes.accessed.unwrap(),
        modified: attributes.modified.unwrap(),
    }
}

async fn restore_times_of(
    generation_handle: &FilesystemGenerationHandle,
    path: &str,
    accessed: TimeChange,
    modified: TimeChange,
) {
    let target = PathTarget::at_root(generation_handle, path).unwrap();
    restore_times(
        generation_handle,
        Target::Path(&target, Follow::No),
        TimeChanges { accessed, modified },
    )
    .unwrap()
    .await
    .unwrap();
}

async fn open_existing(
    generation_handle: &FilesystemGenerationHandle,
    path: &str,
    expected: ObjectKind,
    access: AccessMode,
) -> OpenNode {
    let target = PathTarget::at_root(generation_handle, path).unwrap();
    open(
        generation_handle,
        target,
        OpenOptions::Existing {
            expected,
            access,
            follow: Follow::No,
        },
    )
    .unwrap()
    .await
    .unwrap()
    .node
}

/// Starts an agent with the files `a.txt` and `b.txt`, both 4 bytes long.
async fn two_file_agent(
    agents: &UnmanagedAgents,
    name: &str,
) -> (OwnedAgentId, ResidentFilesystem) {
    let agent = agents.agent(name);
    let filesystem = agents.start(&agent, &[], NO_RESTORE).await.unwrap();
    let generation_handle = resident_generation_handle(&filesystem);
    let at = |path: &str| PathTarget::at_root(&generation_handle, path);
    assert_eq!(
        write_file(&generation_handle, at("a.txt"), b"aaaa").await,
        StepOutcome::Done
    );
    assert_eq!(
        write_file(&generation_handle, at("b.txt"), b"bbbb").await,
        StepOutcome::Done
    );
    (agent, filesystem)
}

#[test]
#[timeout("60s")]
async fn calls_that_read_leave_the_tree_unchanged_across_two_captures_in_a_row() {
    let agents = UnmanagedAgents::new().await;
    let (_, filesystem) = two_file_agent(&agents, "reads").await;
    let generation_handle = resident_generation_handle(&filesystem);
    let at = |path: &str| PathTarget::at_root(&generation_handle, path).unwrap();
    assert_eq!(
        namespace_edit(
            &generation_handle,
            Ok(NamespaceEdit::Insert {
                destination: at("link"),
                object: NewObject::Symlink(SymlinkTarget(PathBuf::from("a.txt"))),
            }),
        )
        .await,
        StepOutcome::Done
    );
    let mark = first_mark(&filesystem).await;

    let node = open_existing(
        &generation_handle,
        "a.txt",
        ObjectKind::File,
        AccessMode::Read,
    )
    .await;
    let OpenNode::File(file) = &node else {
        unreachable!()
    };
    let read = read_file(
        &generation_handle,
        file,
        ReadRange {
            offset: 0,
            length: 4,
        },
    )
    .unwrap()
    .await
    .unwrap();
    let other = open_existing(
        &generation_handle,
        "a.txt",
        ObjectKind::File,
        AccessMode::Read,
    )
    .await;
    let same = is_same_object(&generation_handle, &node, &other)
        .unwrap()
        .await
        .unwrap();
    flush(&generation_handle, &node, FlushLevel::Data)
        .unwrap()
        .await
        .unwrap();
    close(other).await.unwrap();
    close(node).await.unwrap();
    let root = open_existing(
        &generation_handle,
        "",
        ObjectKind::Directory,
        AccessMode::Read,
    )
    .await;
    let OpenNode::Directory(directory) = &root else {
        unreachable!()
    };
    let entries = list_directory(&generation_handle, directory)
        .unwrap()
        .await
        .unwrap();
    close(root).await.unwrap();
    let target = symlink_target(&generation_handle, at("link"))
        .unwrap()
        .await
        .unwrap();
    let _ = times_of(&generation_handle, "b.txt").await;

    let first = look(&filesystem, mark).await;
    let second = look(&filesystem, first.1).await;

    assert_eq!(read.as_ref(), b"aaaa");
    assert!(same);
    assert_eq!(entries.len(), 3);
    assert_eq!(target, SymlinkTarget(PathBuf::from("a.txt")));
    assert_eq!((first.0, second.0), (Seen::Unchanged, Seen::Unchanged));
    assert_eq!(first.1, mark);
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn a_live_stat_and_a_change_of_the_access_time_only_are_no_change() {
    let agents = UnmanagedAgents::new().await;
    let (_, filesystem) = two_file_agent(&agents, "live-stat").await;
    let generation_handle = resident_generation_handle(&filesystem);
    let mark = first_mark(&filesystem).await;
    let times = times_of(&generation_handle, "a.txt").await;

    // A live stat restores the times that it read, which the file already has.
    restore_times_of(
        &generation_handle,
        "a.txt",
        TimeChange::Set(times.accessed),
        TimeChange::Set(times.modified),
    )
    .await;
    let after_same_times = look(&filesystem, mark).await.0;
    restore_times_of(
        &generation_handle,
        "a.txt",
        TimeChange::Set(times.accessed - Duration::from_secs(3600)),
        TimeChange::Set(times.modified),
    )
    .await;
    let after_access_time = look(&filesystem, mark).await.0;

    assert_eq!(
        (after_same_times, after_access_time),
        (Seen::Unchanged, Seen::Unchanged)
    );
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn an_open_for_write_without_a_write_and_a_size_change_to_the_same_size_are_no_change() {
    let agents = UnmanagedAgents::new().await;
    let (_, filesystem) = two_file_agent(&agents, "open-for-write").await;
    let generation_handle = resident_generation_handle(&filesystem);
    let at = |path: &str| PathTarget::at_root(&generation_handle, path);
    let mark = first_mark(&filesystem).await;

    let node = open_existing(
        &generation_handle,
        "a.txt",
        ObjectKind::File,
        AccessMode::Write,
    )
    .await;
    close(node).await.unwrap();
    let after_open = look(&filesystem, mark).await.0;
    assert_eq!(
        truncate_file(&generation_handle, at("a.txt"), 4).await,
        StepOutcome::Done
    );
    let after_same_size = look(&filesystem, mark).await.0;
    assert_eq!(
        truncate_file(&generation_handle, at("a.txt"), 2).await,
        StepOutcome::Done
    );
    let after_shorter = look(&filesystem, mark).await.0;

    assert_eq!(
        (after_open, after_same_size, after_shorter),
        (Seen::Unchanged, Seen::Unchanged, Seen::SizeMtime)
    );
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn calls_whose_times_the_kernel_gives_give_size_and_mtime_detection() {
    let agents = UnmanagedAgents::new().await;
    let (_, filesystem) = two_file_agent(&agents, "fresh").await;
    let generation_handle = resident_generation_handle(&filesystem);
    let at = |path: &str| PathTarget::at_root(&generation_handle, path);
    let mark = first_mark(&filesystem).await;

    assert_eq!(
        write_file(&generation_handle, at("a.txt"), b"AAAA").await,
        StepOutcome::Done
    );
    let (after_write, next) = look(&filesystem, mark).await;
    let (after_nothing, _) = look(&filesystem, next).await;
    assert_eq!(
        set_times_now(&generation_handle, at("b.txt")).await,
        StepOutcome::Done
    );
    let (after_now, next) = look(&filesystem, next).await;
    assert_eq!(
        namespace_edit(
            &generation_handle,
            Ok(NamespaceEdit::Insert {
                destination: at("directory").unwrap(),
                object: NewObject::Directory,
            }),
        )
        .await,
        StepOutcome::Done
    );
    let (after_insert, next) = look(&filesystem, next).await;
    assert_eq!(
        namespace_edit(
            &generation_handle,
            Ok(NamespaceEdit::Remove {
                target: at("b.txt").unwrap(),
                expected: ObjectKind::File,
            }),
        )
        .await,
        StepOutcome::Done
    );
    let (after_remove, _) = look(&filesystem, next).await;

    assert_eq!(
        (
            after_write,
            after_nothing,
            after_now,
            after_insert,
            after_remove
        ),
        (
            Seen::SizeMtime,
            Seen::Unchanged,
            Seen::SizeMtime,
            Seen::SizeMtime,
            Seen::SizeMtime
        )
    );
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn a_rename_a_hard_link_and_a_set_of_a_chosen_time_give_full_detection() {
    let agents = UnmanagedAgents::new().await;
    let (_, filesystem) = two_file_agent(&agents, "carried").await;
    let generation_handle = resident_generation_handle(&filesystem);
    let at = |path: &str| PathTarget::at_root(&generation_handle, path).unwrap();
    let mark = first_mark(&filesystem).await;

    namespace_edit(
        &generation_handle,
        Ok(NamespaceEdit::Move {
            source: at("a.txt"),
            destination: at("b.txt"),
        }),
    )
    .await;
    let (after_rename, renamed) = look(&filesystem, mark).await;
    namespace_edit(
        &generation_handle,
        Ok(NamespaceEdit::Link {
            source: at("b.txt"),
            destination: at("c.txt"),
        }),
    )
    .await;
    let (after_link, linked) = look(&filesystem, renamed).await;
    let times = times_of(&generation_handle, "b.txt").await;
    let set = set_attributes(
        &generation_handle,
        Target::Path(&at("b.txt"), Follow::No),
        AttributeChanges::Times(TimeChanges {
            accessed: TimeChange::Keep,
            modified: TimeChange::Set(times.modified - Duration::from_secs(3600)),
        }),
    )
    .unwrap()
    .await;
    let (after_set, _) = look(&filesystem, linked).await;

    assert!(set.is_ok(), "{set:?}");
    assert_eq!(
        (after_rename, after_link, after_set),
        (Seen::Full, Seen::Full, Seen::Full)
    );
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn a_write_admitted_before_the_capture_and_run_after_it_started_is_in_the_capture() {
    let agents = UnmanagedAgents::new().await;
    let (_, filesystem) = two_file_agent(&agents, "late-write").await;
    let generation_handle = resident_generation_handle(&filesystem);
    let mark = first_mark(&filesystem).await;
    let node = open_existing(
        &generation_handle,
        "a.txt",
        ObjectKind::File,
        AccessMode::Write,
    )
    .await;
    let OpenNode::File(file) = &node else {
        unreachable!()
    };
    let pending = write(
        &generation_handle,
        file,
        WritePlacement::At(0),
        Bytes::from_static(b"LATE"),
    )
    .unwrap();

    let capturing = tokio::spawn(capture(&filesystem, WAIT, Some(mark)));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let waited = !capturing.is_finished();
    pending.await.unwrap();
    let outcome = capturing.await.unwrap().unwrap();

    let CaptureOutcome::Captured {
        capture, detection, ..
    } = outcome
    else {
        panic!("the capture saw no change")
    };
    let copied = std::fs::read(capture.directory().join("tree/a.txt")).unwrap();
    assert_eq!(
        (waited, detection, copied.as_slice()),
        (true, ChangeDetection::SizeMtime, b"LATE".as_slice())
    );
    capture.discard().await.unwrap();
    close(node).await.unwrap();
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn a_write_that_nobody_drives_makes_the_capture_busy_and_never_unchanged() {
    let agents = UnmanagedAgents::new().await;
    let (_, filesystem) = two_file_agent(&agents, "undriven-write").await;
    let generation_handle = resident_generation_handle(&filesystem);
    let mark = first_mark(&filesystem).await;
    let node = open_existing(
        &generation_handle,
        "a.txt",
        ObjectKind::File,
        AccessMode::Write,
    )
    .await;
    let OpenNode::File(file) = &node else {
        unreachable!()
    };
    let pending = write(
        &generation_handle,
        file,
        WritePlacement::At(0),
        Bytes::from_static(b"LATE"),
    )
    .unwrap();

    let outcome = capture(&filesystem, Duration::from_millis(100), Some(mark)).await;

    assert!(matches!(outcome, Err(CaptureError::Busy)));
    drop(pending);
    // The dropped write never ran, but it counted as a change when it took its lease.
    assert_eq!(look(&filesystem, mark).await.0, Seen::SizeMtime);
    close(node).await.unwrap();
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn a_save_after_a_failed_upload_compares_with_the_older_mark_and_sees_every_change() {
    let agents = UnmanagedAgents::new().await;
    let (_, filesystem) = two_file_agent(&agents, "failed-upload").await;
    let generation_handle = resident_generation_handle(&filesystem);
    let at = |path: &str| PathTarget::at_root(&generation_handle, path);
    let confirmed = first_mark(&filesystem).await;
    assert_eq!(
        write_file(&generation_handle, at("a.txt"), b"AAAA").await,
        StepOutcome::Done
    );
    // The upload of this capture fails, so the caller keeps the confirmed mark.
    let (_, failed) = look(&filesystem, confirmed).await;

    let against_confirmed = look(&filesystem, confirmed).await.0;
    let against_failed = look(&filesystem, failed).await.0;

    assert_eq!(
        (against_confirmed, against_failed),
        (Seen::SizeMtime, Seen::Unchanged)
    );
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn an_initial_file_update_and_a_provision_give_full_detection_even_when_nothing_changes() {
    let agents = UnmanagedAgents::new().await;
    let store = &agents.store;
    let v1 = [store
        .declare("/config.txt", AgentFilePermissions::ReadWrite, b"one")
        .await];
    let v2 = [store
        .declare("/config.txt", AgentFilePermissions::ReadWrite, b"two")
        .await];
    let provisioned = [store
        .declare("/tool.txt", AgentFilePermissions::ReadOnly, b"tool")
        .await];
    let agent = agents.agent("initial-file-update");
    let filesystem = agents.start(&agent, &v1, NO_RESTORE).await.unwrap();
    let generation_handle = resident_generation_handle(&filesystem);
    let update = |files: &[InitialAgentFile]| {
        update_initial_files(
            &generation_handle,
            Arc::clone(&store.loader),
            store.environment_id,
            files.to_vec(),
        )
        .unwrap()
    };
    let mark = first_mark(&filesystem).await;

    update(&v1).await.unwrap();
    let (same, same_mark) = look(&filesystem, mark).await;
    update(&v2).await.unwrap();
    let (changed, changed_mark) = look(&filesystem, same_mark).await;
    provision_initial_files(
        &generation_handle,
        Arc::clone(&store.loader),
        store.environment_id,
        provisioned.to_vec(),
    )
    .unwrap()
    .await
    .unwrap();
    let (after_provision, _) = look(&filesystem, changed_mark).await;

    assert_eq!(
        (same, changed, after_provision),
        (Seen::Full, Seen::Full, Seen::Full)
    );
    assert_eq!(
        std::fs::read(agents.root(&agent).join("config.txt")).unwrap(),
        b"two"
    );
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn a_restored_tree_is_unchanged_against_its_baseline_mark_until_a_change() {
    let agents = UnmanagedAgents::new().await;
    let (_, source) = two_file_agent(&agents, "restore-source").await;
    let captured = copy_of(&source).await;
    let agent = agents.agent("restored");
    let restored = agents
        .start(&agent, &[], Some(copying_restore(&captured)))
        .await
        .unwrap();
    let generation_handle = resident_generation_handle(&restored);
    let at = |path: &str| PathTarget::at_root(&generation_handle, path);
    let baseline = tree_mark(&restored);

    let untouched = look(&restored, baseline).await.0;
    // A replayed stat of a restored file sets the times that the capture recorded, which the
    // restore already put there.
    let times = times_of(&generation_handle, "a.txt").await;
    restore_times_of(
        &generation_handle,
        "a.txt",
        TimeChange::Keep,
        TimeChange::Set(times.modified),
    )
    .await;
    let after_replayed_stat = look(&restored, baseline).await.0;
    assert_eq!(
        write_file(&generation_handle, at("b.txt"), b"BBBB").await,
        StepOutcome::Done
    );
    let after_write = look(&restored, baseline).await.0;

    assert_eq!(
        (untouched, after_replayed_stat, after_write),
        (Seen::Unchanged, Seen::Unchanged, Seen::SizeMtime)
    );
    captured.discard().await.unwrap();
    delete(seal(source)).await.unwrap();
    delete(seal(restored)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn a_replayed_stat_that_puts_back_a_past_time_after_a_replayed_write_gives_full_detection() {
    let agents = UnmanagedAgents::new().await;
    let (_, source) = two_file_agent(&agents, "replay-source").await;
    let captured = copy_of(&source).await;
    let agent = agents.agent("replayed");
    let restored = agents
        .start(&agent, &[], Some(copying_restore(&captured)))
        .await
        .unwrap();
    let generation_handle = resident_generation_handle(&restored);
    let at = |path: &str| PathTarget::at_root(&generation_handle, path);
    let baseline = tree_mark(&restored);
    let recorded = times_of(&generation_handle, "a.txt").await;

    // The replay writes the file again now, and the replayed stat then puts back the time that the
    // first run saw.
    assert_eq!(
        write_file(&generation_handle, at("a.txt"), b"AAAA").await,
        StepOutcome::Done
    );
    restore_times_of(
        &generation_handle,
        "a.txt",
        TimeChange::Keep,
        TimeChange::Set(recorded.modified),
    )
    .await;

    assert_eq!(look(&restored, baseline).await.0, Seen::Full);
    captured.discard().await.unwrap();
    delete(seal(source)).await.unwrap();
    delete(seal(restored)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn a_restore_that_the_initial_file_rule_changes_is_unchanged_but_never_size_and_mtime() {
    let agents = UnmanagedAgents::new().await;
    let store = &agents.store;
    let v1 = [store
        .declare("/config.txt", AgentFilePermissions::ReadWrite, b"one")
        .await];
    let v2 = [store
        .declare("/config.txt", AgentFilePermissions::ReadWrite, b"two")
        .await];
    let source_agent = agents.agent("update-source");
    let source = agents.start(&source_agent, &v1, NO_RESTORE).await.unwrap();
    let captured = copy_of(&source).await;
    let agent = agents.agent("updated");
    let updated = agents
        .start(&agent, &v2, Some(copying_restore(&captured)))
        .await
        .unwrap();
    let generation_handle = resident_generation_handle(&updated);
    let at = |path: &str| PathTarget::at_root(&generation_handle, path);
    let baseline = tree_mark(&updated);

    let untouched = look(&updated, baseline).await.0;
    assert_eq!(
        write_file(&generation_handle, at("new.txt"), b"new").await,
        StepOutcome::Done
    );
    let after_write = look(&updated, baseline).await.0;

    assert_eq!((untouched, after_write), (Seen::Unchanged, Seen::Full));
    captured.discard().await.unwrap();
    delete(seal(source)).await.unwrap();
    delete(seal(updated)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn a_mark_of_another_generation_never_matches() {
    let agents = UnmanagedAgents::new().await;
    let (agent, first) = two_file_agent(&agents, "generations").await;
    let mark = first_mark(&first).await;
    delete(seal(first)).await.unwrap();
    let again = agents.start(&agent, &[], NO_RESTORE).await.unwrap();
    let generation_handle = resident_generation_handle(&again);
    assert_eq!(
        write_file(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "a.txt"),
            b"aaaa"
        )
        .await,
        StepOutcome::Done
    );

    assert!(!tree_mark(&again).same_generation(&mark));
    assert_eq!(look(&again, mark).await.0, Seen::Full);
    delete(seal(again)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn a_capture_after_a_change_keeps_the_calls_stopped_until_the_times_settle() {
    let agents = UnmanagedAgents::new().await;
    let (_, filesystem) = two_file_agent(&agents, "settle").await;
    let generation_handle = resident_generation_handle(&filesystem);
    let at = |path: &str| PathTarget::at_root(&generation_handle, path);
    let mark = first_mark(&filesystem).await;
    assert_eq!(
        write_file(&generation_handle, at("a.txt"), b"AAAA").await,
        StepOutcome::Done
    );
    let written = std::time::Instant::now();

    let _ = look(&filesystem, mark).await;

    assert!(written.elapsed() >= Duration::from_millis(15));
    delete(seal(filesystem)).await.unwrap();
}

/// Starts an agent with read-only initial files at `a.txt` and `nested/deeper/b.txt`.
async fn read_only_agent(
    agents: &UnmanagedAgents,
    name: &str,
    extra: &[InitialAgentFile],
) -> (OwnedAgentId, ResidentFilesystem) {
    let store = &agents.store;
    let files = [
        store
            .declare("/a.txt", AgentFilePermissions::ReadOnly, b"first")
            .await,
        store
            .declare(
                "/nested/deeper/b.txt",
                AgentFilePermissions::ReadOnly,
                b"second",
            )
            .await,
    ]
    .into_iter()
    .chain(extra.iter().cloned())
    .collect::<Vec<_>>();
    let agent = agents.agent(name);
    let filesystem = agents.start(&agent, &files, NO_RESTORE).await.unwrap();
    (agent, filesystem)
}

#[test]
#[timeout("60s")]
async fn an_empty_tree_without_initial_files_makes_no_copy_and_no_host_directory() {
    let agents = UnmanagedAgents::new().await;
    let agent = agents.agent("empty");
    let filesystem = agents.start(&agent, &[], NO_RESTORE).await.unwrap();

    let seen = look_without_mark(&filesystem).await;

    assert_eq!(seen, Seen::InitialFiles);
    assert!(scratch_is_empty(&agents.scratch));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn untouched_read_only_initial_files_make_no_copy_and_no_host_directory() {
    let agents = UnmanagedAgents::new().await;
    let (_, filesystem) = read_only_agent(&agents, "untouched", &[]).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let _ = times_of(&generation_handle, "nested/deeper/b.txt").await;

    let seen = look_without_mark(&filesystem).await;

    assert_eq!(seen, Seen::InitialFiles);
    assert!(scratch_is_empty(&agents.scratch));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("60s")]
async fn an_update_that_keeps_every_directory_keeps_a_tree_of_initial_files() {
    let agents = UnmanagedAgents::new().await;
    let (_, filesystem) = read_only_agent(&agents, "updated-read-only", &[]).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let store = &agents.store;
    let v2 = [
        store
            .declare("/a.txt", AgentFilePermissions::ReadOnly, b"changed")
            .await,
        store
            .declare(
                "/nested/deeper/b.txt",
                AgentFilePermissions::ReadOnly,
                b"second",
            )
            .await,
        store
            .declare("/c.txt", AgentFilePermissions::ReadOnly, b"third")
            .await,
    ];

    update_initial_files(
        &generation_handle,
        Arc::clone(&store.loader),
        store.environment_id,
        v2.to_vec(),
    )
    .unwrap()
    .await
    .unwrap();

    let kept = look_without_mark(&filesystem).await;
    // An update that removes the only file of a directory keeps the directory, which a start from
    // the initial files does not make.
    let v3 = [store
        .declare("/a.txt", AgentFilePermissions::ReadOnly, b"changed")
        .await];
    update_initial_files(
        &generation_handle,
        Arc::clone(&store.loader),
        store.environment_id,
        v3.to_vec(),
    )
    .unwrap()
    .await
    .unwrap();
    let emptied = look_without_mark(&filesystem).await;

    assert_eq!((kept, emptied), (Seen::InitialFiles, Seen::Full));
    delete(seal(filesystem)).await.unwrap();
}

/// What a change to a tree of read-only initial files does to the next capture.
async fn capture_after(name: &str, extra: &[(&str, AgentFilePermissions)], change: Change) -> Seen {
    let agents = UnmanagedAgents::new().await;
    let store = &agents.store;
    let extra = futures::stream::iter(extra)
        .then(|(path, permissions)| store.declare(path, *permissions, b"extra"))
        .collect::<Vec<_>>()
        .await;
    let (_, filesystem) = read_only_agent(&agents, name, &extra).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let at = |path: &str| PathTarget::at_root(&generation_handle, path).unwrap();
    let outcome = match change {
        Change::None => StepOutcome::Done,
        Change::Provision => {
            let provisioned = [store
                .declare("/tool.txt", AgentFilePermissions::ReadOnly, b"tool")
                .await];
            outcome_of(
                provision_initial_files(
                    &generation_handle,
                    Arc::clone(&store.loader),
                    store.environment_id,
                    provisioned.to_vec(),
                )
                .unwrap()
                .await,
            )
        }
        Change::Delete => {
            namespace_edit(
                &generation_handle,
                Ok(NamespaceEdit::Remove {
                    target: at("a.txt"),
                    expected: ObjectKind::File,
                }),
            )
            .await
        }
        Change::Rename => {
            namespace_edit(
                &generation_handle,
                Ok(NamespaceEdit::Move {
                    source: at("a.txt"),
                    destination: at("moved.txt"),
                }),
            )
            .await
        }
        Change::Link => {
            namespace_edit(
                &generation_handle,
                Ok(NamespaceEdit::Link {
                    source: at("a.txt"),
                    destination: at("linked.txt"),
                }),
            )
            .await
        }
        Change::AgentFile => {
            write_file(
                &generation_handle,
                PathTarget::at_root(&generation_handle, "nested/agent.txt"),
                b"agent",
            )
            .await
        }
        Change::EmptyDirectory => {
            namespace_edit(
                &generation_handle,
                Ok(NamespaceEdit::Insert {
                    destination: at("nested/empty"),
                    object: NewObject::Directory,
                }),
            )
            .await
        }
        Change::Symlink => {
            namespace_edit(
                &generation_handle,
                Ok(NamespaceEdit::Insert {
                    destination: at("link"),
                    object: NewObject::Symlink(SymlinkTarget(PathBuf::from("a.txt"))),
                }),
            )
            .await
        }
        Change::DirectoryTime => {
            let set = set_attributes(
                &generation_handle,
                Target::Path(&at("nested"), Follow::No),
                AttributeChanges::Times(TimeChanges {
                    accessed: TimeChange::Keep,
                    modified: TimeChange::Set(
                        std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000),
                    ),
                }),
            )
            .unwrap()
            .await;
            outcome_of(set)
        }
        Change::MoveAndBack => {
            let away = namespace_edit(
                &generation_handle,
                Ok(NamespaceEdit::Move {
                    source: at("nested"),
                    destination: at("elsewhere"),
                }),
            )
            .await;
            assert_eq!(away, StepOutcome::Done);
            namespace_edit(
                &generation_handle,
                Ok(NamespaceEdit::Move {
                    source: at("elsewhere"),
                    destination: at("nested"),
                }),
            )
            .await
        }
        Change::TemporaryFile => {
            let written = write_file(
                &generation_handle,
                PathTarget::at_root(&generation_handle, "temporary.txt"),
                b"gone",
            )
            .await;
            assert_eq!(written, StepOutcome::Done);
            namespace_edit(
                &generation_handle,
                Ok(NamespaceEdit::Remove {
                    target: at("temporary.txt"),
                    expected: ObjectKind::File,
                }),
            )
            .await
        }
    };
    assert_eq!(outcome, StepOutcome::Done, "{name}");
    let seen = look_without_mark(&filesystem).await;
    delete(seal(filesystem)).await.unwrap();
    seen
}

/// A name, the extra declarations, a change, and what the capture after the change sees.
type ChangeCase = (
    &'static str,
    &'static [(&'static str, AgentFilePermissions)],
    Change,
    Seen,
);

#[derive(Clone, Copy, Debug)]
enum Change {
    None,
    Provision,
    Delete,
    Rename,
    Link,
    AgentFile,
    EmptyDirectory,
    Symlink,
    DirectoryTime,
    MoveAndBack,
    TemporaryFile,
}

#[test]
#[timeout("120s")]
async fn each_change_of_a_tree_of_initial_files_gives_a_copy() {
    const READ_WRITE: &[(&str, AgentFilePermissions)] =
        &[("/config.txt", AgentFilePermissions::ReadWrite)];
    let cases: [ChangeCase; 12] = [
        ("untouched", &[], Change::None, Seen::InitialFiles),
        // A kernel time on a directory comes back from a replay as a new time too.
        (
            "temporary-file",
            &[],
            Change::TemporaryFile,
            Seen::InitialFiles,
        ),
        ("read-write", READ_WRITE, Change::None, Seen::Full),
        ("provisioned", &[], Change::Provision, Seen::Full),
        ("deleted", &[], Change::Delete, Seen::Full),
        ("renamed", &[], Change::Rename, Seen::Full),
        ("linked", &[], Change::Link, Seen::Full),
        ("agent-file", &[], Change::AgentFile, Seen::Full),
        ("empty-directory", &[], Change::EmptyDirectory, Seen::Full),
        ("symlink", &[], Change::Symlink, Seen::Full),
        ("directory-time", &[], Change::DirectoryTime, Seen::Full),
        ("move-and-back", &[], Change::MoveAndBack, Seen::Full),
    ];

    let seen = futures::stream::iter(cases.iter())
        .then(|(name, extra, change, _)| async move {
            (*name, capture_after(name, extra, *change).await)
        })
        .collect::<Vec<_>>()
        .await;

    assert_eq!(
        seen,
        cases
            .iter()
            .map(|(name, _, _, expected)| (*name, *expected))
            .collect::<Vec<_>>()
    );
}

#[test]
#[timeout("60s")]
async fn a_restored_tree_of_initial_files_is_never_a_tree_of_initial_files() {
    let agents = UnmanagedAgents::new().await;
    let (_, source) = read_only_agent(&agents, "restore-source", &[]).await;
    let source_handle = resident_generation_handle(&source);
    assert_eq!(
        write_file(
            &source_handle,
            PathTarget::at_root(&source_handle, "agent.txt"),
            b"agent"
        )
        .await,
        StepOutcome::Done
    );
    let captured = copy_of(&source).await;
    let restored_agent = agents.agent("restored");
    let declared = [
        agents
            .store
            .declare("/a.txt", AgentFilePermissions::ReadOnly, b"first")
            .await,
        agents
            .store
            .declare(
                "/nested/deeper/b.txt",
                AgentFilePermissions::ReadOnly,
                b"second",
            )
            .await,
    ];
    let restored = agents
        .start(&restored_agent, &declared, Some(copying_restore(&captured)))
        .await
        .unwrap();
    let generation_handle = resident_generation_handle(&restored);
    assert_eq!(
        namespace_edit(
            &generation_handle,
            Ok(NamespaceEdit::Remove {
                target: PathTarget::at_root(&generation_handle, "agent.txt").unwrap(),
                expected: ObjectKind::File,
            }),
        )
        .await,
        StepOutcome::Done
    );

    // The restored directories hold the times of the save, which a start without the restore
    // does not give.
    assert_eq!(look_without_mark(&restored).await, Seen::Full);
    captured.discard().await.unwrap();
    delete(seal(source)).await.unwrap();
    delete(seal(restored)).await.unwrap();
}
