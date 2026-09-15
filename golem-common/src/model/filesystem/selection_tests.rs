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
fn file_read_selection_consumes_shared_range_vectors() {
    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    ))
    .unwrap();
    let ids = [
        "file-range-bounded",
        "file-range-open-ended",
        "file-range-suffix",
        "file-range-clipped",
        "file-range-at-length",
        "file-range-empty-file",
        "file-range-zero-suffix",
        "file-head-ignores-range",
    ];
    for id in ids {
        let case = corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == id)
            .unwrap_or_else(|| panic!("Missing shared case {id}"));
        let input = &case["input"];
        // Translate only these valid fixture ranges to generic inputs. HTTP parsing and HEAD
        // dispatch are not implemented or certified by this test.
        let selection = if input["method"] == "HEAD" {
            assert_eq!(case["expect"]["read_selection"], "MetadataOnly", "{id}");
            FileByteSelection::MetadataOnly
        } else {
            let range = input["headers"][0][1].as_str().unwrap();
            let (start, end) = range
                .strip_prefix("bytes=")
                .unwrap()
                .split_once('-')
                .unwrap();
            match (start, end) {
                ("", suffix) => FileByteSelection::Suffix {
                    length: suffix.parse().unwrap(),
                },
                (start, "") => FileByteSelection::OpenEnded {
                    start: start.parse().unwrap(),
                },
                (start, end) => FileByteSelection::Bounded {
                    start: start.parse().unwrap(),
                    end_inclusive: end.parse().unwrap(),
                },
            }
        };
        let body = input["body_hex"].as_str().unwrap();
        let total_size = (body.len() / 2) as u64;
        let result = selection.resolve(total_size).unwrap();
        let selected_hex = match result {
            FileReadExtent::Selected { offset, length } => {
                assert_ne!(case["expect"]["status"], 416, "{id}");
                &body[(offset * 2) as usize..((offset + length) * 2) as usize]
            }
            FileReadExtent::Unsatisfiable => {
                assert_eq!(case["expect"]["status"], 416, "{id}");
                ""
            }
        };
        assert_eq!(
            selected_hex,
            case["expect"]["body_hex"].as_str().unwrap(),
            "{id}"
        );
        let metadata = FileReadMetadata {
            total_size,
            selection: result,
            modified_at: None,
        };
        metadata.validate_for(selection).unwrap();
        let head = FileReadHead::File(metadata);
        assert_eq!(
            FileReadHead::try_from(proto::FileReadHead::from(head.clone())).unwrap(),
            head,
            "{id}"
        );
    }
}

#[test]
fn file_read_selection_handles_u64_boundaries_without_file_allocation() {
    use FileByteSelection::*;
    use FileReadExtent::{Selected, Unsatisfiable};
    for (selection, size, expected) in [
        (
            Full,
            0,
            Selected {
                offset: 0,
                length: 0,
            },
        ),
        (
            Full,
            u64::MAX,
            Selected {
                offset: 0,
                length: u64::MAX,
            },
        ),
        (
            MetadataOnly,
            u64::MAX,
            Selected {
                offset: 0,
                length: 0,
            },
        ),
        (
            Bounded {
                start: 7,
                end_inclusive: u64::MAX,
            },
            u64::MAX,
            Selected {
                offset: 7,
                length: u64::MAX - 7,
            },
        ),
        (
            Bounded {
                start: u64::MAX - 1,
                end_inclusive: u64::MAX,
            },
            u64::MAX,
            Selected {
                offset: u64::MAX - 1,
                length: 1,
            },
        ),
        (OpenEnded { start: u64::MAX }, u64::MAX, Unsatisfiable),
        (
            OpenEnded {
                start: u64::MAX - 2,
            },
            u64::MAX,
            Selected {
                offset: u64::MAX - 2,
                length: 2,
            },
        ),
        (
            Suffix { length: u64::MAX },
            3,
            Selected {
                offset: 0,
                length: 3,
            },
        ),
        (
            Suffix { length: 2 },
            u64::MAX,
            Selected {
                offset: u64::MAX - 2,
                length: 2,
            },
        ),
        (Suffix { length: 0 }, u64::MAX, Unsatisfiable),
        (Suffix { length: 1 }, 0, Unsatisfiable),
    ] {
        assert_eq!(
            selection.resolve(size).unwrap(),
            expected,
            "{selection:?} size {size}"
        );
        assert_eq!(
            FileByteSelection::try_from(proto::FileByteSelection::from(selection)).unwrap(),
            selection
        );
    }
    for size in [0, 3, u64::MAX] {
        let invalid = Bounded {
            start: 2,
            end_inclusive: 1,
        };
        assert_eq!(invalid.resolve(size), Err(FileReadError::InvalidSelection));
        assert_eq!(
            FileByteSelection::try_from(proto::FileByteSelection::from(invalid)),
            Err(FileReadError::InvalidSelection)
        );
    }
    assert_eq!(
        FileByteSelection::try_from(proto::FileByteSelection { selection: None }),
        Err(FileReadError::InvalidSelection)
    );
}

#[test]
fn file_read_metadata_rejects_malformed_or_wrong_selection() {
    let metadata = FileReadMetadata {
        total_size: 9,
        selection: FileReadExtent::Selected {
            offset: 7,
            length: 2,
        },
        modified_at: None,
    };
    metadata
        .validate_for(FileByteSelection::Suffix { length: 2 })
        .unwrap();
    for selection in [
        FileByteSelection::Full,
        FileByteSelection::MetadataOnly,
        FileByteSelection::OpenEnded { start: 8 },
    ] {
        assert_eq!(
            metadata.validate_for(selection),
            Err(FileReadError::InvalidResponse)
        );
    }
    for (metadata, selection) in [
        (
            FileReadMetadata {
                total_size: 9,
                selection: FileReadExtent::Unsatisfiable,
                modified_at: None,
            },
            FileByteSelection::Full,
        ),
        (
            FileReadMetadata {
                total_size: 3,
                selection: FileReadExtent::Selected {
                    offset: 3,
                    length: 0,
                },
                modified_at: None,
            },
            FileByteSelection::OpenEnded { start: 3 },
        ),
    ] {
        assert_eq!(
            metadata.validate_for(selection),
            Err(FileReadError::InvalidResponse)
        );
    }
    for (size, offset, length) in [(3, 4, 0), (3, 2, 2), (u64::MAX, u64::MAX - 1, 2)] {
        let head = proto::FileReadHead {
            outcome: Some(proto::file_read_head::Outcome::File(
                proto::FileReadMetadata {
                    total_size: size,
                    selection: Some(proto::file_read_metadata::Selection::Selected(
                        proto::FileReadExtent { offset, length },
                    )),
                    modified_at: None,
                },
            )),
        };
        assert_eq!(
            FileReadHead::try_from(head),
            Err(FileReadError::InvalidResponse)
        );
    }
    for head in [
        proto::FileReadHead { outcome: None },
        proto::FileReadHead {
            outcome: Some(proto::file_read_head::Outcome::File(
                proto::FileReadMetadata {
                    total_size: 0,
                    selection: None,
                    modified_at: None,
                },
            )),
        },
    ] {
        assert_eq!(
            FileReadHead::try_from(head),
            Err(FileReadError::InvalidResponse)
        );
    }
}

#[test]
fn file_read_heads_preserve_lookup_status_and_timestamp() {
    for head in [
        FileReadHead::Absent,
        FileReadHead::NotRegular,
        FileReadHead::Symlink,
        FileReadHead::PermissionDenied,
    ] {
        assert_eq!(
            FileReadHead::try_from(proto::FileReadHead::from(head.clone())).unwrap(),
            head
        );
    }
    for (seconds, nanos) in [
        (-62_135_596_800, 0),
        (-1, 123_456_789),
        (0, 0),
        (253_402_300_799, 999_999_999),
    ] {
        let head = FileReadHead::File(FileReadMetadata {
            total_size: 4,
            selection: FileReadExtent::Selected {
                offset: 1,
                length: 3,
            },
            modified_at: DateTime::from_timestamp(seconds, nanos),
        });
        assert_eq!(
            FileReadHead::try_from(proto::FileReadHead::from(head.clone())).unwrap(),
            head
        );
    }
    for (seconds, nanos) in [
        (0, -1),
        (0, 1_000_000_000),
        (i64::MAX, 0),
        (-62_135_596_801, 0),
        (253_402_300_800, 0),
    ] {
        let head = proto::FileReadHead {
            outcome: Some(proto::file_read_head::Outcome::File(
                proto::FileReadMetadata {
                    total_size: 0,
                    selection: Some(proto::file_read_metadata::Selection::Unsatisfiable(
                        Default::default(),
                    )),
                    modified_at: Some(prost_types::Timestamp { seconds, nanos }),
                },
            )),
        };
        assert_eq!(
            FileReadHead::try_from(head),
            Err(FileReadError::InvalidResponse)
        );
    }
}

#[test]
fn file_read_heads_omit_unrepresentable_optional_timestamp() {
    for (seconds, nanos) in [
        (253_402_300_800, 0),
        (-62_135_596_801, 0),
        (59, 1_000_000_000),
    ] {
        let metadata = FileReadMetadata {
            total_size: 7,
            selection: FileReadExtent::Selected {
                offset: 2,
                length: 5,
            },
            modified_at: Some(DateTime::from_timestamp(seconds, nanos).unwrap()),
        };
        let wire = proto::FileReadHead::from(FileReadHead::File(metadata.clone()));
        assert_eq!(
            FileReadHead::try_from(wire).unwrap(),
            FileReadHead::File(FileReadMetadata {
                modified_at: None,
                ..metadata
            }),
        );
    }
}
