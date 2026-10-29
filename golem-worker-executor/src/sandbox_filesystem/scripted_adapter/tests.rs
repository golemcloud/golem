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

use crate::host_directories::HostDirectory;
use crate::host_directories::HostPath;
use crate::sandbox_filesystem::InstalledLimits;
use crate::sandbox_filesystem::SandboxAccessMode;
use crate::sandbox_filesystem::SandboxAttributes;
use crate::sandbox_filesystem::SandboxDirectoryEntry;
use crate::sandbox_filesystem::SandboxFilesystemAdapter;
use crate::sandbox_filesystem::SandboxFlushLevel;
use crate::sandbox_filesystem::SandboxFollow;
use crate::sandbox_filesystem::SandboxObjectKind;
use crate::sandbox_filesystem::SandboxOpenOptions;
use crate::sandbox_filesystem::SandboxReadRange;
use crate::sandbox_filesystem::SandboxSymlinkTarget;
use crate::sandbox_filesystem::SandboxTimeChange;
use crate::sandbox_filesystem::SandboxTimeChanges;
use crate::sandbox_filesystem::SandboxWriteAttempt;
use crate::sandbox_filesystem::SandboxWritePlacement;
use crate::sandbox_filesystem::SeedAccess;
use crate::sandbox_filesystem::SeedEntry;
use crate::sandbox_filesystem::SeedPlacement;
use crate::sandbox_filesystem::errors::FilesystemStorageError;
use crate::sandbox_filesystem::host_adapter::sandbox_provisioning::SandboxFilesystemName;
use crate::sandbox_filesystem::host_adapter::tree_copy::TreeExclusions;
use crate::sandbox_filesystem::object_identity::SandboxObjectId;
use crate::sandbox_filesystem::open_handles::SandboxDirectory;
use crate::sandbox_filesystem::open_handles::SandboxFile;
use crate::sandbox_filesystem::open_handles::SandboxHandle;
use crate::sandbox_filesystem::sandbox_paths::SandboxPath;
use crate::sandbox_filesystem::scripted_adapter::ScriptedSandboxFilesystem;
use crate::sandbox_filesystem::scripted_adapter::ScriptedSandboxFilesystemProvisioning;
use crate::services::golem_config::FilesystemStorageMode;
use crate::storage_volume::FilesystemAllocation;
use crate::storage_volume::FilesystemLimits;
use bytes::Bytes;
use golem_common::model::RetryConfig;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use test_r::test;

async fn create_scripted_filesystem(
    provisioning: ScriptedSandboxFilesystemProvisioning,
) -> ScriptedSandboxFilesystem {
    <ScriptedSandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap()
}

fn host_child_path(directory: &HostDirectory, name: &str) -> HostPath {
    directory.path().child(std::ffi::OsStr::new(name)).unwrap()
}

/// Makes the host directories under `root` on development storage, and gives `.scratch`.
async fn create_scratch_directory(root: &Path) -> HostDirectory {
    crate::services::agent_filesystem::provision_executor_directories(
        &FilesystemStorageMode::Directory { root: root.into() },
        RetryConfig::default(),
    )
    .await
    .unwrap()
    .1
    .scratch
}

fn sandbox_name() -> SandboxFilesystemName {
    SandboxFilesystemName::new(
        "environment".to_string(),
        "component".to_string(),
        "filesystem".to_string(),
    )
    .unwrap()
}

fn scripted_storage_error(operation: &'static str) -> FilesystemStorageError {
    FilesystemStorageError::verification_without_path(operation)
}

fn make_seed_entry(
    source: HostPath,
    target: impl Into<PathBuf>,
    access: SeedAccess,
    placement: SeedPlacement,
) -> SeedEntry {
    SeedEntry {
        source,
        target: SandboxPath::at_root(target),
        access,
        placement,
    }
}

#[test]
async fn scripted_adapter_returns_programmed_outcomes_and_records_exact_order() {
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    let seed_parent = tempfile::tempdir().unwrap();
    let seed_directory = create_scratch_directory(seed_parent.path()).await;
    let seed_source = host_child_path(&seed_directory, "seed-source");
    let seed_call = format!(
        "seed(entries=[{{source={}, target=SandboxPath {{ base: Root, path: \"seed-destination\" }}, access=ReadWrite, placement=CreateNew}}])",
        seed_source.as_path().display()
    );
    let file = SandboxFile::scripted(7);
    let directory = SandboxDirectory::scripted(9);
    let attributes = SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 1,
        size: 12,
        accessed: None,
        modified: None,
        read_only: false,
        object: SandboxObjectId::scripted(12),
    };
    let limits = FilesystemLimits {
        allocated_bytes: 4096,
        filesystem_objects: 8,
    };
    let allocation = FilesystemAllocation {
        allocated_bytes: 1024,
        filesystem_objects: 3,
    };
    control.push_open(Ok(SandboxHandle::scripted_file(7)));
    control.push_read(Ok(Bytes::from_static(b"programmed")));
    control.push_read_directory(Ok(vec![SandboxDirectoryEntry {
        name: OsString::from("entry"),
        kind: SandboxObjectKind::Directory,
    }]));
    control.push_read_link(Ok(SandboxSymlinkTarget(PathBuf::from("target"))));
    control.push_write(Ok(SandboxWriteAttempt::completed(4)));
    control.push_get_attributes(Ok(attributes.clone()));
    control.push_set_size(Ok(()));
    control.push_set_times(Ok(()));
    control.push_create_directory(Ok(()));
    control.push_create_symlink(Ok(()));
    control.push_hard_link(Ok(()));
    control.push_rename(Ok(()));
    control.push_remove_directory(Ok(()));
    control.push_unlink_file(Ok(()));
    control.push_flush(Ok(()));
    control.push_close(Ok(()));
    control.push_seed(Ok(()));
    control.push_observe_allocation(Ok(allocation));
    control.push_install_limits(Ok(InstalledLimits { limits, allocation }));
    control.push_delete_and_verify(Ok(()));

    let filesystem = create_scripted_filesystem(provisioning).await;
    let opened = filesystem
        .open(
            SandboxPath::at_root("opened"),
            SandboxOpenOptions::Existing {
                expected: SandboxObjectKind::File,
                access: SandboxAccessMode::Read,
                follow: SandboxFollow::Yes,
            },
        )
        .await
        .unwrap();
    assert!(matches!(opened, SandboxHandle::File(_)));
    assert_eq!(
        filesystem
            .read(
                &file,
                SandboxReadRange {
                    offset: 2,
                    length: 10,
                },
            )
            .await
            .unwrap(),
        Bytes::from_static(b"programmed")
    );
    assert_eq!(
        filesystem.read_directory(&directory).await.unwrap(),
        vec![SandboxDirectoryEntry {
            name: OsString::from("entry"),
            kind: SandboxObjectKind::Directory,
        }]
    );
    assert_eq!(
        filesystem
            .read_link(SandboxPath::at(directory.clone(), "link"))
            .await
            .unwrap(),
        SandboxSymlinkTarget(PathBuf::from("target"))
    );
    let attempt = filesystem
        .write(
            &file,
            SandboxWritePlacement::At(3),
            Bytes::from_static(b"data"),
        )
        .await
        .unwrap();
    assert_eq!(attempt.written, 4);
    assert!(attempt.result.is_ok());
    assert_eq!(
        filesystem
            .get_handle_attributes(SandboxHandle::File(file.clone()))
            .await
            .unwrap(),
        attributes
    );
    filesystem.set_size(&file, 20).await.unwrap();
    filesystem
        .set_handle_times(
            SandboxHandle::File(file.clone()),
            SandboxTimeChanges {
                accessed: SandboxTimeChange::Keep,
                modified: SandboxTimeChange::Now,
            },
        )
        .await
        .unwrap();
    filesystem
        .create_directory(SandboxPath::at(directory.clone(), "new-dir"))
        .await
        .unwrap();
    filesystem
        .create_symlink(
            SandboxPath::at(directory.clone(), "new-link"),
            SandboxSymlinkTarget(PathBuf::from("target")),
        )
        .await
        .unwrap();
    filesystem
        .hard_link(
            SandboxPath::at(directory.clone(), "source"),
            SandboxPath::at(directory.clone(), "hard"),
        )
        .await
        .unwrap();
    filesystem
        .rename(
            SandboxPath::at(directory.clone(), "before"),
            SandboxPath::at(directory.clone(), "after"),
        )
        .await
        .unwrap();
    filesystem
        .remove_directory(SandboxPath::at(directory.clone(), "old-dir"))
        .await
        .unwrap();
    filesystem
        .unlink_file(SandboxPath::at(directory.clone(), "old-file"))
        .await
        .unwrap();
    filesystem
        .flush(
            &SandboxHandle::File(file.clone()),
            SandboxFlushLevel::DataAndMetadata,
        )
        .await
        .unwrap();
    filesystem
        .close(SandboxHandle::File(file.clone()))
        .await
        .unwrap();
    filesystem
        .seed(Box::new([make_seed_entry(
            seed_source,
            "seed-destination",
            SeedAccess::ReadWrite,
            SeedPlacement::CreateNew,
        )]))
        .await
        .unwrap();
    assert_eq!(filesystem.observe_allocation().await.unwrap(), allocation);
    assert_eq!(
        filesystem.install_limits(limits).await.unwrap(),
        InstalledLimits { limits, allocation }
    );
    <ScriptedSandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();

    assert_eq!(
        control.calls(),
        vec![
            "create_fresh(name=environment/component/filesystem, limits=None)",
            "open(target=SandboxPath { base: Root, path: \"opened\" }, options=Existing { expected: File, access: Read, follow: Yes })",
            "read(file=file(7), range=SandboxReadRange { offset: 2, length: 10 })",
            "read_directory(directory=directory(9))",
            "read_link(path=SandboxPath { base: Directory(directory(9)), path: \"link\" })",
            "write(file=file(7), placement=At(3), bytes=b\"data\")",
            "get_handle_attributes(node=File(file(7)))",
            "set_size(file=file(7), size=20)",
            "set_handle_times(node=File(file(7)), times=SandboxTimeChanges { accessed: Keep, modified: Now })",
            "create_directory(path=SandboxPath { base: Directory(directory(9)), path: \"new-dir\" })",
            "create_symlink(path=SandboxPath { base: Directory(directory(9)), path: \"new-link\" }, target=SandboxSymlinkTarget(\"target\"))",
            "hard_link(source=SandboxPath { base: Directory(directory(9)), path: \"source\" }, destination=SandboxPath { base: Directory(directory(9)), path: \"hard\" })",
            "rename(source=SandboxPath { base: Directory(directory(9)), path: \"before\" }, destination=SandboxPath { base: Directory(directory(9)), path: \"after\" })",
            "remove_directory(path=SandboxPath { base: Directory(directory(9)), path: \"old-dir\" })",
            "unlink_file(path=SandboxPath { base: Directory(directory(9)), path: \"old-file\" })",
            "flush(node=File(file(7)), level=DataAndMetadata)",
            "close(node=File(file(7)))",
            seed_call.as_str(),
            "observe_allocation()",
            "install_limits(limits=FilesystemLimits { allocated_bytes: 4096, filesystem_objects: 8 })",
            "delete_and_verify()",
        ]
    );
}

#[test]
async fn scripted_adapter_surfaces_each_programmed_native_outcome() {
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    control.push_read(Ok(Bytes::from_static(b"first")));
    control.push_read(Err(scripted_storage_error("changed read outcome")));
    let filesystem = create_scripted_filesystem(provisioning).await;
    let file = SandboxFile::scripted(1);
    let range = SandboxReadRange {
        offset: 0,
        length: 5,
    };

    assert_eq!(
        filesystem.read(&file, range).await.unwrap(),
        Bytes::from_static(b"first")
    );
    assert_eq!(
        filesystem.read(&file, range).await.unwrap_err().to_string(),
        "failed to changed read outcome filesystem <scripted-test>"
    );
    assert_eq!(
        control.calls(),
        vec![
            "create_fresh(name=environment/component/filesystem, limits=None)",
            "read(file=file(1), range=SandboxReadRange { offset: 0, length: 5 })",
            "read(file=file(1), range=SandboxReadRange { offset: 0, length: 5 })",
        ]
    );
}

#[test]
async fn scripted_namespace_resolution_exposes_only_opaque_semantic_facts() {
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    control.push_path_resolution(41, "equivalent", Some(42));
    control.push_path_resolution(41, "equivalent", Some(42));
    let filesystem = create_scripted_filesystem(provisioning).await;

    let first = filesystem
        .resolve_path_target(SandboxPath::at_root("First"))
        .await
        .unwrap();
    let second = filesystem
        .resolve_path_target(SandboxPath::at_root("second"))
        .await
        .unwrap();

    assert!(first.name_comparison_key() == second.name_comparison_key());
    assert!(first.target_directory_id() == second.target_directory_id());
    assert_eq!(first.target().path, PathBuf::from("First"));
    assert!(
        !first.has_byte_exact_lookup_proof(),
        "a canonical coordination label is not a lookup proof"
    );
    assert!(!second.has_byte_exact_lookup_proof());

    let default = filesystem
        .resolve_path_target(SandboxPath::at_root("default"))
        .await
        .unwrap();
    assert!(default.has_byte_exact_lookup_proof());
    control.push_path_resolution(41, "First", Some(42));
    let same_label = filesystem
        .resolve_path_target(SandboxPath::at_root("First"))
        .await
        .unwrap();
    assert!(
        !same_label.has_byte_exact_lookup_proof(),
        "matching a canonical label does not prove the parent's lookup"
    );
}

#[test]
async fn scripted_adapter_programs_creation_through_associated_provisioning() {
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    control.set_creation(Err(scripted_storage_error("programmed creation failure")));

    let error = <ScriptedSandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .expect_err("scripted creation must return the programmed failure");

    assert_eq!(
        error.to_string(),
        "failed to programmed creation failure filesystem <scripted-test>"
    );
    assert_eq!(
        control.calls(),
        vec!["create_fresh(name=environment/component/filesystem, limits=None)"]
    );
}

#[test]
async fn scripted_adapter_programs_copy_contents_with_gates() {
    let parent = tempfile::tempdir().unwrap();
    let copies = create_scratch_directory(parent.path()).await;
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    control.push_copy_contents(Ok(Box::new([])));
    control.push_copy_contents(Err(scripted_storage_error("programmed copy failure")));
    let filesystem = create_scripted_filesystem(provisioning).await;
    let target = host_child_path(&copies, "copy");
    let excluded = Arc::new(TreeExclusions::new([
        PathBuf::from("lib/b.txt"),
        PathBuf::from("a.txt"),
    ]));

    let gate = control.block("copy_contents");
    let copying = tokio::spawn({
        let filesystem = filesystem.clone();
        let excluded = Arc::clone(&excluded);
        let target = target.clone();
        async move {
            filesystem
                .copy_contents(SandboxPath::at_root("data"), excluded, &target)
                .await
        }
    });
    gate.wait_started().await;
    assert!(!copying.is_finished());
    gate.release();
    gate.wait_completed().await;
    copying.await.unwrap().unwrap();
    let failed = filesystem
        .copy_contents(
            SandboxPath::at_root(""),
            Arc::new(TreeExclusions::default()),
            &target,
        )
        .await
        .unwrap_err();
    let unprogrammed = filesystem
        .copy_contents(SandboxPath::at_root(""), excluded, &target)
        .await
        .unwrap_err();

    assert_eq!(
        failed.to_string(),
        "failed to programmed copy failure filesystem <scripted-test>"
    );
    assert_eq!(
        unprogrammed.to_string(),
        "failed to consume programmed scripted sandbox filesystem outcome filesystem <scripted-sandbox-filesystem>"
    );
    let target_path = target.as_path().display();
    assert_eq!(
        control.calls(),
        vec![
            "create_fresh(name=environment/component/filesystem, limits=None)".to_string(),
            format!(
                "copy_contents(source=SandboxPath {{ base: Root, path: \"data\" }}, excluded=[\"a.txt\", \"lib/b.txt\"], target={target_path})"
            ),
            format!(
                "copy_contents(source=SandboxPath {{ base: Root, path: \"\" }}, excluded=[], target={target_path})"
            ),
            format!(
                "copy_contents(source=SandboxPath {{ base: Root, path: \"\" }}, excluded=[\"a.txt\", \"lib/b.txt\"], target={target_path})"
            ),
        ]
    );
}

#[test]
async fn scripted_adapter_programs_seed_with_gates() {
    let parent = tempfile::tempdir().unwrap();
    let sources = create_scratch_directory(parent.path()).await;
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    control.push_seed(Ok(()));
    control.push_seed(Err(scripted_storage_error("programmed seed failure")));
    let filesystem = create_scripted_filesystem(provisioning).await;
    let entries = || -> Box<[SeedEntry]> {
        Box::new([
            make_seed_entry(
                host_child_path(&sources, "tree"),
                "",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
            make_seed_entry(
                host_child_path(&sources, "file"),
                "lib/file",
                SeedAccess::ReadOnly,
                SeedPlacement::Replace,
            ),
        ])
    };

    let gate = control.block("seed");
    let seeding = tokio::spawn({
        let filesystem = filesystem.clone();
        let entries = entries();
        async move { filesystem.seed(entries).await }
    });
    gate.wait_started().await;
    assert!(!seeding.is_finished());
    gate.release();
    gate.wait_completed().await;
    seeding.await.unwrap().unwrap();
    let failed = filesystem.seed(entries()).await.unwrap_err();
    let unprogrammed = filesystem.seed(entries()).await.unwrap_err();

    assert_eq!(
        failed.to_string(),
        "failed to programmed seed failure filesystem <scripted-test>"
    );
    assert_eq!(
        unprogrammed.to_string(),
        "failed to consume programmed scripted sandbox filesystem outcome filesystem <scripted-sandbox-filesystem>"
    );
    let call = format!(
        "seed(entries=[{{source={}, target=SandboxPath {{ base: Root, path: \"\" }}, access=FromSource, placement=CreateNew}}, {{source={}, target=SandboxPath {{ base: Root, path: \"lib/file\" }}, access=ReadOnly, placement=Replace}}])",
        sources.path().as_path().join("tree").display(),
        sources.path().as_path().join("file").display()
    );
    assert_eq!(
        control.calls(),
        vec![
            "create_fresh(name=environment/component/filesystem, limits=None)".to_string(),
            call.clone(),
            call.clone(),
            call,
        ]
    );
}
