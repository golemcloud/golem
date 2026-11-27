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

use super::parse_oplog;
use test_r::test;

#[test]
fn oplog_query_combines_with_a_range_and_the_file_export_excludes_the_other_outputs() {
    assert!(parse_oplog(&["--query", "error", "--from", "3", "--to", "9"]).is_ok());
    assert!(parse_oplog(&["--to", "9", "--format", "jsonl", "--output", "f.jsonl"]).is_ok());
    [
        vec!["--output", "f.jsonl", "--page"],
        vec!["--output", "f.jsonl", "--html", "f.html"],
        vec!["--output", "f.jsonl", "--serve"],
        vec!["--output", "f.jsonl", "--obfuscate"],
        vec!["--output", "f.jsonl", "--omit-payloads"],
        vec!["--to", "9", "--obfuscate"],
        vec!["--to", "9", "--html", "f.html"],
        vec!["--to", "9", "--serve"],
    ]
    .into_iter()
    .for_each(|conflicting| {
        assert_eq!(
            parse_oplog(&conflicting).unwrap_err().kind(),
            clap::error::ErrorKind::ArgumentConflict,
            "{conflicting:?}"
        )
    });
}
