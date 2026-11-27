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
fn an_export_file_gets_the_mode_of_a_plain_write() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain.jsonl");
    std::fs::write(&plain, "x").unwrap();

    let new = dir.path().join("nested/new.jsonl");
    export_temporary_file(&new).unwrap().persist(&new).unwrap();
    assert_eq!(mode(&new), mode(&plain));

    let existing = dir.path().join("existing.jsonl");
    std::fs::write(&existing, "old").unwrap();
    std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o644)).unwrap();
    export_temporary_file(&existing)
        .unwrap()
        .persist(&existing)
        .unwrap();
    assert_eq!(mode(&existing), 0o644);
}
