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

use super::SnapshotName;
use pretty_assertions::assert_eq;
use test_r::test;

#[test]
fn a_snapshot_name_has_1_to_64_ascii_letters_digits_dashes_or_underscores() {
    let accepted = [
        "p-8c1e3b0a-4c64-4e1e-9d4f-2f1c9a7b6e55".to_string(),
        "u-8c1e3b0a-4c64-4e1e-9d4f-2f1c9a7b6e55".to_string(),
        "a".to_string(),
        "Z_9".to_string(),
        "x".repeat(64),
    ];
    let refused = [
        String::new(),
        "x".repeat(65),
        "a/b".to_string(),
        ".".to_string(),
        "..".to_string(),
        "é".to_string(),
        " a".to_string(),
        "a.b".to_string(),
    ];

    assert_eq!(
        (
            accepted
                .iter()
                .map(|text| SnapshotName::new(text).map(|name| name.as_str().to_string()))
                .collect::<Vec<_>>(),
            refused
                .iter()
                .map(|text| SnapshotName::new(text).is_err())
                .collect::<Vec<_>>()
        ),
        (
            accepted.iter().cloned().map(Ok).collect::<Vec<_>>(),
            refused.iter().map(|_| true).collect::<Vec<_>>()
        )
    );
}
