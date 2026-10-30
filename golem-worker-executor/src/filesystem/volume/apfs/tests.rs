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
use crate::services::golem_config::FilesystemStorageMode;
use golem_common::model::RetryConfig;
use test_r::test;

fn assert_bind_keeps_clone_and_cleanup_failures(errno: i32) {
    use std::cell::Cell;
    use std::os::unix::fs::FileExt as _;

    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("development/root").into_boxed_path();
    assert!(!root.exists());
    let clone_attempts = Cell::new(0);
    let close_attempts = Cell::new(0);
    let result = bind_with_probe(
        &root,
        |source, directory, name| {
            clone_attempts.set(clone_attempts.get() + 1);
            assert!(root.is_dir());
            assert!(directory.metadata().unwrap().is_dir());
            assert_eq!(name, Path::new("clone"));
            let mut contents = [0; 4096];
            source.read_exact_at(&mut contents, 0).unwrap();
            assert_eq!(contents, [0x5a; 4096]);
            Err(std::io::Error::from_raw_os_error(errno))
        },
        |probe| {
            close_attempts.set(close_attempts.get() + 1);
            assert_eq!(probe.path().parent(), Some(root.as_ref()));
            assert!(probe.path().join("source").is_file());
            assert!(!probe.path().join("clone").exists());
            probe.close()?;
            Err(std::io::Error::from_raw_os_error(libc::EIO))
        },
    );
    let Err(error) = result else {
        panic!("bind must not return a volume after the clone probe fails");
    };
    assert_eq!(clone_attempts.get(), 1);
    assert_eq!(close_attempts.get(), 1);
    assert!(root.is_dir());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    assert_eq!(
        error.as_ref().context().io_error().unwrap().raw_os_error(),
        Some(libc::EIO)
    );
    let original = std::io::Error::from_raw_os_error(errno);
    let cleanup = std::io::Error::from_raw_os_error(libc::EIO);
    assert_eq!(
        error.to_string(),
        format!(
            "failed to probe APFS clone for local development at {}: {original}; cleanup also failed: failed to remove APFS clone probe at {}: {cleanup}",
            root.display(),
            root.display()
        )
    );
    assert_eq!(error.as_ref().context().io_kind(), Some(cleanup.kind()));
    assert!(error.as_ref().cleanup_failed());
    assert!(error.as_ref().context().task_error().is_none());
}

#[test]
#[test_r::tag(native_apfs)]
#[ignore = "requires the native APFS runner's non-cloning filesystem fixture"]
fn apfs_constructor_refuses_a_root_on_a_non_cloning_filesystem() {
    let parent = std::env::var_os("GOLEM_APFS_NON_CLONE_ROOT")
        .expect("the native APFS runner supplies the non-cloning filesystem root");
    let root = tempfile::tempdir_in(parent).unwrap();
    let error = FilesystemVolume::open(
        &FilesystemStorageMode::Apfs {
            root: Box::from(root.path()),
        },
        RetryConfig::default(),
    )
    .err()
    .expect("a non-cloning filesystem cannot provide APFS storage");
    assert!(
        error
            .to_string()
            .contains("probe APFS clone for local development")
    );
    assert!(error.to_string().contains(root.path().to_str().unwrap()));
    assert_eq!(
        error.as_ref().context().io_error().unwrap().raw_os_error(),
        Some(libc::ENOTSUP),
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
#[test_r::tag(native_apfs)]
fn apfs_failed_clone_probe_reports_the_root_and_removes_its_files() {
    let root = tempfile::tempdir().unwrap();
    let error = probe_clone(
        root.path(),
        |_, _, _| Err(std::io::Error::from_raw_os_error(libc::ENOTSUP)),
        tempfile::TempDir::close,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("probe APFS clone for local development")
    );
    assert!(error.to_string().contains(root.path().to_str().unwrap()));
    assert_eq!(
        error.as_ref().context().io_kind(),
        Some(std::io::Error::from_raw_os_error(libc::ENOTSUP).kind())
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    assert!(!error.as_ref().cleanup_failed());
}

#[test]
#[test_r::tag(native_apfs)]
fn apfs_bind_enotsup_clone_and_cleanup_failures_keep_both_causes() {
    assert_bind_keeps_clone_and_cleanup_failures(libc::ENOTSUP);
}

#[test]
#[test_r::tag(native_apfs)]
fn apfs_bind_exdev_clone_and_cleanup_failures_keep_both_causes() {
    assert_bind_keeps_clone_and_cleanup_failures(libc::EXDEV);
}

#[test]
#[test_r::tag(native_apfs)]
fn apfs_successful_probe_with_failed_removal_reports_the_cleanup_cause() {
    use std::os::unix::fs::FileExt as _;
    let root = tempfile::tempdir().unwrap();
    let error = probe_clone(
        root.path(),
        |source, directory, name| {
            let parent = cap_std::fs::Dir::from_std_file(directory.try_clone().unwrap());
            let mut target = parent.create(name)?;
            let mut contents = [0; 4096];
            source.read_exact_at(&mut contents, 0)?;
            target.write_all(&contents)?;
            Ok(())
        },
        |probe| {
            probe.close()?;
            Err(std::io::ErrorKind::PermissionDenied.into())
        },
    )
    .unwrap_err();
    assert!(error.as_ref().cleanup_failed());
    assert!(
        error
            .to_string()
            .starts_with("failed to remove APFS clone probe")
    );
    assert_eq!(
        error.as_ref().context().io_kind(),
        Some(std::io::ErrorKind::PermissionDenied)
    );
    assert!(!error.to_string().contains("cleanup also failed"));
}

#[test]
#[test_r::tag(native_apfs)]
fn apfs_clone_across_system_and_data_volumes_fails() {
    let source = File::open("/System/Library/CoreServices/SystemVersion.plist").unwrap();
    let target = tempfile::tempdir().unwrap();
    let parent = File::open(target.path()).unwrap();
    let error = clone_file_at(&source, &parent, Path::new("clone")).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EXDEV));
    assert!(!target.path().join("clone").exists());
}

#[test]
#[test_r::tag(native_apfs)]
fn apfs_space_uses_the_fragment_size_and_the_free_counts_and_refuses_a_read_only_volume() {
    let capacity = |f_flag| StatVfs {
        f_bsize: 1_048_576,
        f_frsize: 4096,
        f_blocks: 10,
        f_bfree: 6,
        f_bavail: 4,
        f_files: 20,
        f_ffree: 9,
        f_favail: 7,
        f_fsid: 0,
        f_flag,
        f_namemax: 255,
    };

    assert_eq!(
        space_from_capacity(&capacity(StatVfsMountFlags::NOSUID)).unwrap(),
        FilesystemSpace::Observed {
            total_bytes: 40_960,
            available_bytes: 16_384,
            total_filesystem_objects: 20,
            available_filesystem_objects: 9,
        }
    );
    assert_eq!(
        space_from_capacity(&capacity(
            StatVfsMountFlags::RDONLY | StatVfsMountFlags::NOSUID
        ))
        .unwrap_err()
        .kind(),
        std::io::ErrorKind::ReadOnlyFilesystem
    );
}
