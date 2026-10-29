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
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use test_r::test;

struct Directories {
    scratch: HostDirectory,
    initial_files: HostDirectory,
}

async fn provision(root: Option<&Path>) -> (Option<Arc<tempfile::TempDir>>, Directories) {
    let temporary = root
        .is_none()
        .then(|| Arc::new(tempfile::tempdir().unwrap()));
    let root = root.unwrap_or_else(|| temporary.as_ref().unwrap().path());
    let scratch = HostDirectory::create_fresh(root.join(".scratch"), None, temporary.clone())
        .await
        .unwrap();
    let initial_files =
        HostDirectory::create_fresh(root.join(".initial-files"), None, temporary.clone())
            .await
            .unwrap();
    (
        temporary,
        Directories {
            scratch,
            initial_files,
        },
    )
}
async fn scratch_at(root: &Path) -> HostDirectory {
    provision(Some(root)).await.1.scratch
}
fn is_empty_directory(path: &Path) -> bool {
    std::fs::read_dir(path).unwrap().next().is_none()
}
#[cfg(unix)]
fn running_as_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}
#[test]
async fn a_temporary_root_lives_while_a_host_directory_lives() {
    let (provisioning, directories) = provision(None).await;
    let Directories {
        scratch,
        initial_files,
    } = directories;

    let root = scratch.path().as_path().parent().unwrap().to_path_buf();
    assert_ne!(root, std::env::temp_dir());
    assert_eq!(
        initial_files.path().as_path().parent(),
        Some(root.as_path())
    );
    drop(provisioning);
    drop(initial_files);
    assert!(
        scratch.path().as_path().is_dir(),
        "a host directory must keep the temporary root after the provisioning is dropped"
    );
    drop(scratch);
    assert!(
        !root.exists(),
        "the temporary root must go away with its last host directory"
    );
}

#[test]
async fn create_in_refuses_an_existing_name() {
    let root = tempfile::tempdir().unwrap();
    let parent = scratch_at(root.path()).await;
    std::fs::write(parent.path().as_path().join("file"), b"").unwrap();

    let child = HostDirectory::create_in(parent.path(), OsStr::new("child"))
        .await
        .unwrap();
    let existing_directory = HostDirectory::create_in(parent.path(), OsStr::new("child"))
        .await
        .unwrap_err();
    let existing_file = HostDirectory::create_in(parent.path(), OsStr::new("file"))
        .await
        .unwrap_err();
    let invalid = HostDirectory::create_in(parent.path(), OsStr::new("a/b"))
        .await
        .unwrap_err();

    assert_eq!(child.path().as_path(), root.path().join(".scratch/child"));
    assert!(child.path().as_path().is_dir());
    assert_eq!(existing_directory.io_kind(), Some(ErrorKind::AlreadyExists));
    assert_eq!(existing_file.io_kind(), Some(ErrorKind::AlreadyExists));
    assert_eq!(invalid.io_kind(), Some(ErrorKind::InvalidInput));
}

#[test]
async fn child_refuses_a_name_that_is_not_one_normal_component() {
    let root = tempfile::tempdir().unwrap();
    let directory = scratch_at(root.path()).await;

    ["", ".", "..", "a/b", "/a", "a/"]
        .into_iter()
        .for_each(|name| {
            assert_eq!(
                directory
                    .path()
                    .child(OsStr::new(name))
                    .unwrap_err()
                    .io_kind(),
                Some(ErrorKind::InvalidInput),
                "{name:?} must be refused"
            );
        });
    assert_eq!(
        directory.path().child(OsStr::new("a")).unwrap().as_path(),
        root.path().join(".scratch/a")
    );
    assert_eq!(
        directory.path().child(OsStr::new(".a")).unwrap().as_path(),
        root.path().join(".scratch/.a")
    );
}

#[test]
async fn discard_and_drop_remove_the_directory_with_its_contents() {
    let root = tempfile::tempdir().unwrap();
    let (_provisioning, directories) = provision(Some(root.path())).await;
    let Directories {
        scratch: discarded,
        initial_files: dropped,
    } = directories;
    std::fs::create_dir(discarded.path().as_path().join("nested")).unwrap();
    std::fs::write(discarded.path().as_path().join("nested/file"), b"data").unwrap();
    std::fs::create_dir(dropped.path().as_path().join("nested")).unwrap();
    std::fs::write(dropped.path().as_path().join("nested/file"), b"data").unwrap();

    discarded.discard().await.unwrap();
    drop(dropped);

    assert!(is_empty_directory(root.path()));
}

#[test]
async fn a_child_removed_with_its_parent_discards_without_error() {
    let root = tempfile::tempdir().unwrap();
    let (_provisioning, directories) = provision(Some(root.path())).await;
    let child = HostDirectory::create_in(directories.scratch.path(), OsStr::new("child"))
        .await
        .unwrap();
    std::fs::write(child.path().as_path().join("file"), b"data").unwrap();

    drop(directories);

    child.discard().await.unwrap();
    assert!(is_empty_directory(root.path()));
}

#[cfg(unix)]
#[test]
async fn discard_reports_a_removal_failure() {
    if running_as_root() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let directory = scratch_at(root.path()).await;
    let locked = directory.path().as_path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::write(locked.join("file"), b"contents").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();

    let error = directory.discard().await.unwrap_err();

    assert!(error.cleanup_failed(), "{error}");
    assert!(
        error
            .to_string()
            .starts_with("failed to discard host directory"),
        "{error}"
    );
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(locked.join("file").is_file());
}
