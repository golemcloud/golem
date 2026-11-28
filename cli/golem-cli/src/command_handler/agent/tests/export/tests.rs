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

use super::super::check_oplog_output_format;
use pretty_assertions::assert_eq;
use test_r::test;

#[test]
fn an_oplog_file_export_needs_the_jsonl_format() {
    use crate::model::format::Format;
    use strum::IntoEnumIterator;
    let rows = [
        (Format::Json, false),
        (Format::Jsonl, true),
        (Format::PrettyJson, false),
        (Format::Yaml, false),
        (Format::PrettyYaml, false),
        (Format::Toon, false),
        (Format::Text, false),
    ];
    assert_eq!(
        rows.iter().map(|(format, _)| *format).collect::<Vec<_>>(),
        Format::iter().collect::<Vec<_>>(),
        "the table has one row for each format"
    );
    let output = std::path::Path::new("oplog.jsonl");
    rows.into_iter().for_each(|(format, accepted)| {
        assert_eq!(
            check_oplog_output_format(Some(output), format).is_ok(),
            accepted,
            "--output with {format}"
        );
        assert!(
            check_oplog_output_format(None, format).is_ok(),
            "no --output with {format}"
        );
    });
}
