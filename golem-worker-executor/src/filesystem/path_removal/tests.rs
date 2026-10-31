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
use test_r::test;

#[test]
fn removing_a_missing_path_file_or_directory_verifies_absence() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("removed");
    remove_and_verify(&path).unwrap();
    std::fs::write(&path, b"file").unwrap();
    remove_and_verify(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("file"), b"nested").unwrap();
    remove_and_verify(&path).unwrap();
    assert!(!path.exists());
}

#[cfg(unix)]
#[test]
fn removing_a_link_does_not_remove_its_target() {
    let parent = tempfile::tempdir().unwrap();
    let target = parent.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("file"), b"kept").unwrap();
    let link = parent.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    remove_and_verify(&link).unwrap();
    assert!(!link.exists());
    assert_eq!(std::fs::read(target.join("file")).unwrap(), b"kept");
}
