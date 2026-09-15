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

use crate::base_model::agent::http_files::valid_decoded_segment;
use chrono::{DateTime, Utc};
use golem_api_grpc::proto::golem::worker as proto;

#[cfg(test)]
mod selection_tests;

pub const FILE_READ_CHUNK_SIZE: usize = 64 * 1024;

/// A filesystem target, independent of URI syntax and public route mappings.
///
/// Validate before joining any paths. The executor filesystem boundary must validate again,
/// even when a trusted caller or a protobuf conversion has already checked the target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileReadTarget {
    Exact {
        file_path: String,
    },
    WithinRoot {
        root: String,
        suffix: Vec<String>,
        directory_request: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InvalidFileReadTarget {
    #[error("File read target must be a canonical absolute filesystem path")]
    InvalidPath,
    #[error("File read suffix must contain only safe nonempty path segments")]
    InvalidSuffix,
    #[error("File read target is missing")]
    MissingTarget,
}

impl FileReadTarget {
    /// Checks raw filesystem text without normalizing it or percent-decoding it.
    ///
    /// An empty subtree suffix is valid and refers to the configured root itself. A directory
    /// request is retained separately so callers cannot silently turn it into a regular-file read.
    pub fn validate(&self) -> Result<(), InvalidFileReadTarget> {
        let (path, allow_root) = match self {
            Self::Exact { file_path } => (file_path.as_str(), false),
            Self::WithinRoot { root, suffix, .. } => {
                if suffix.iter().any(|segment| !valid_decoded_segment(segment)) {
                    return Err(InvalidFileReadTarget::InvalidSuffix);
                }
                (root.as_str(), true)
            }
        };
        if !path.starts_with('/')
            || path.chars().any(char::is_control)
            || (path == "/" && !allow_root)
            || (path != "/"
                && path[1..]
                    .split('/')
                    .any(|part| !valid_decoded_segment(part)))
        {
            return Err(InvalidFileReadTarget::InvalidPath);
        }
        Ok(())
    }
}

impl TryFrom<proto::FileReadTarget> for FileReadTarget {
    type Error = InvalidFileReadTarget;

    fn try_from(value: proto::FileReadTarget) -> Result<Self, Self::Error> {
        use proto::file_read_target::Target;
        let target = match value.target.ok_or(InvalidFileReadTarget::MissingTarget)? {
            Target::Exact(file_path) => Self::Exact { file_path },
            Target::WithinRoot(target) => Self::WithinRoot {
                root: target.root,
                suffix: target.suffix,
                directory_request: target.directory_request,
            },
        };
        target.validate()?;
        Ok(target)
    }
}

impl From<FileReadTarget> for proto::FileReadTarget {
    fn from(value: FileReadTarget) -> Self {
        use proto::file_read_target::Target;
        let target = match value {
            FileReadTarget::Exact { file_path } => Target::Exact(file_path),
            FileReadTarget::WithinRoot {
                root,
                suffix,
                directory_request,
            } => Target::WithinRoot(proto::RootBoundedFileReadTarget {
                root,
                suffix,
                directory_request,
            }),
        };
        Self {
            target: Some(target),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileByteSelection {
    Full,
    Bounded { start: u64, end_inclusive: u64 },
    OpenEnded { start: u64 },
    Suffix { length: u64 },
    MetadataOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileReadExtent {
    Selected { offset: u64, length: u64 },
    Unsatisfiable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FileReadError {
    #[error("Invalid filesystem read target")]
    InvalidTarget,
    #[error("Invalid byte selection")]
    InvalidSelection,
    #[error("Filesystem read admission exhausted")]
    ResourceExhausted,
    #[error("Filesystem read deadline exceeded")]
    DeadlineExceeded,
    #[error("Agent lifecycle operation failed")]
    Lifecycle,
    #[error("Filesystem storage operation failed")]
    Storage,
    #[error("Invalid filesystem read response")]
    InvalidResponse,
}

impl crate::metrics::api::ApiErrorDetails for FileReadError {
    fn trace_error_kind(&self) -> &'static str {
        match self {
            Self::InvalidTarget => "InvalidTarget",
            Self::InvalidSelection => "InvalidSelection",
            Self::ResourceExhausted => "ResourceExhausted",
            Self::DeadlineExceeded => "DeadlineExceeded",
            Self::Lifecycle => "Lifecycle",
            Self::Storage => "Storage",
            Self::InvalidResponse => "InvalidResponse",
        }
    }

    fn is_expected(&self) -> bool {
        matches!(
            self,
            Self::InvalidTarget
                | Self::InvalidSelection
                | Self::ResourceExhausted
                | Self::DeadlineExceeded
        )
    }

    fn take_cause(&mut self) -> Option<anyhow::Error> {
        None
    }
}

impl From<FileReadError> for proto::FileReadError {
    fn from(value: FileReadError) -> Self {
        match value {
            FileReadError::InvalidTarget => Self::InvalidTarget,
            FileReadError::InvalidSelection => Self::InvalidSelection,
            FileReadError::ResourceExhausted => Self::ResourceExhausted,
            FileReadError::DeadlineExceeded => Self::DeadlineExceeded,
            FileReadError::Lifecycle => Self::Lifecycle,
            FileReadError::Storage => Self::Storage,
            FileReadError::InvalidResponse => Self::InvalidResponse,
        }
    }
}

impl TryFrom<i32> for FileReadError {
    type Error = FileReadError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match proto::FileReadError::try_from(value) {
            Ok(proto::FileReadError::InvalidTarget) => Ok(Self::InvalidTarget),
            Ok(proto::FileReadError::InvalidSelection) => Ok(Self::InvalidSelection),
            Ok(proto::FileReadError::ResourceExhausted) => Ok(Self::ResourceExhausted),
            Ok(proto::FileReadError::DeadlineExceeded) => Ok(Self::DeadlineExceeded),
            Ok(proto::FileReadError::Lifecycle) => Ok(Self::Lifecycle),
            Ok(proto::FileReadError::Storage) => Ok(Self::Storage),
            Ok(proto::FileReadError::InvalidResponse) => Ok(Self::InvalidResponse),
            _ => Err(Self::InvalidResponse),
        }
    }
}

impl FileByteSelection {
    pub fn validate(self) -> Result<(), FileReadError> {
        if let Self::Bounded {
            start,
            end_inclusive,
        } = self
            && end_inclusive < start
        {
            return Err(FileReadError::InvalidSelection);
        }
        Ok(())
    }

    /// Resolves a selection against metadata from the opened file without allocating file bytes.
    pub fn resolve(self, total_size: u64) -> Result<FileReadExtent, FileReadError> {
        self.validate()?;
        let (offset, length) = match self {
            Self::Full => (0, total_size),
            Self::MetadataOnly => (0, 0),
            Self::Bounded {
                start,
                end_inclusive,
            } if start < total_size => {
                // Clamp before adding one so even u64::MAX endpoints cannot overflow.
                let end_exclusive = end_inclusive.min(total_size - 1) + 1;
                (start, end_exclusive - start)
            }
            Self::OpenEnded { start } if start < total_size => (start, total_size - start),
            Self::Suffix { length } if length > 0 && total_size > 0 => {
                let length = length.min(total_size);
                (total_size - length, length)
            }
            _ => return Ok(FileReadExtent::Unsatisfiable),
        };
        Ok(FileReadExtent::Selected { offset, length })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileReadMetadata {
    pub total_size: u64,
    pub selection: FileReadExtent,
    pub modified_at: Option<DateTime<Utc>>,
}

impl FileReadMetadata {
    /// Checks the response against the requested selection before any bytes are accepted.
    pub fn validate_for(&self, requested: FileByteSelection) -> Result<(), FileReadError> {
        if requested.resolve(self.total_size)? != self.selection {
            return Err(FileReadError::InvalidResponse);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileReadHead {
    File(FileReadMetadata),
    Absent,
    NotRegular,
    Symlink,
    PermissionDenied,
}

// Protobuf timestamps represent years 1 through 9999 and do not encode leap seconds.
fn proto_timestamp(seconds: i64, nanos: i32) -> Option<prost_types::Timestamp> {
    ((-62_135_596_800..=253_402_300_799).contains(&seconds) && (0..1_000_000_000).contains(&nanos))
        .then_some(prost_types::Timestamp { seconds, nanos })
}

impl TryFrom<proto::FileByteSelection> for FileByteSelection {
    type Error = FileReadError;

    fn try_from(value: proto::FileByteSelection) -> Result<Self, Self::Error> {
        use proto::file_byte_selection::Selection;
        let selection = match value.selection.ok_or(FileReadError::InvalidSelection)? {
            Selection::Full(_) => Self::Full,
            Selection::Bounded(range) => Self::Bounded {
                start: range.start,
                end_inclusive: range.end_inclusive,
            },
            Selection::OpenEnded(start) => Self::OpenEnded { start },
            Selection::Suffix(length) => Self::Suffix { length },
            Selection::MetadataOnly(_) => Self::MetadataOnly,
        };
        selection.validate()?;
        Ok(selection)
    }
}

impl From<FileByteSelection> for proto::FileByteSelection {
    fn from(value: FileByteSelection) -> Self {
        use proto::file_byte_selection::Selection;
        let selection = match value {
            FileByteSelection::Full => Selection::Full(Default::default()),
            FileByteSelection::Bounded {
                start,
                end_inclusive,
            } => Selection::Bounded(proto::BoundedFileByteSelection {
                start,
                end_inclusive,
            }),
            FileByteSelection::OpenEnded { start } => Selection::OpenEnded(start),
            FileByteSelection::Suffix { length } => Selection::Suffix(length),
            FileByteSelection::MetadataOnly => Selection::MetadataOnly(Default::default()),
        };
        Self {
            selection: Some(selection),
        }
    }
}

impl TryFrom<proto::FileReadHead> for FileReadHead {
    type Error = FileReadError;

    fn try_from(value: proto::FileReadHead) -> Result<Self, Self::Error> {
        use proto::file_read_head::Outcome;
        Ok(match value.outcome.ok_or(FileReadError::InvalidResponse)? {
            Outcome::File(metadata) => {
                use proto::file_read_metadata::Selection;
                let selection = match metadata.selection.ok_or(FileReadError::InvalidResponse)? {
                    Selection::Selected(range)
                        if range.offset <= metadata.total_size
                            && range.length <= metadata.total_size - range.offset =>
                    {
                        FileReadExtent::Selected {
                            offset: range.offset,
                            length: range.length,
                        }
                    }
                    Selection::Unsatisfiable(_) => FileReadExtent::Unsatisfiable,
                    _ => return Err(FileReadError::InvalidResponse),
                };
                let modified_at = metadata
                    .modified_at
                    .map(|timestamp| {
                        proto_timestamp(timestamp.seconds, timestamp.nanos)
                            .ok_or(FileReadError::InvalidResponse)?;
                        DateTime::from_timestamp(timestamp.seconds, timestamp.nanos as u32)
                            .ok_or(FileReadError::InvalidResponse)
                    })
                    .transpose()?;
                Self::File(FileReadMetadata {
                    total_size: metadata.total_size,
                    selection,
                    modified_at,
                })
            }
            Outcome::Absent(_) => Self::Absent,
            Outcome::NotRegular(_) => Self::NotRegular,
            Outcome::Symlink(_) => Self::Symlink,
            Outcome::PermissionDenied(_) => Self::PermissionDenied,
        })
    }
}

impl From<FileReadHead> for proto::FileReadHead {
    fn from(value: FileReadHead) -> Self {
        use proto::file_read_head::Outcome;
        let outcome = match value {
            FileReadHead::File(metadata) => {
                use proto::file_read_metadata::Selection;
                let selection = match metadata.selection {
                    FileReadExtent::Selected { offset, length } => {
                        Selection::Selected(proto::FileReadExtent { offset, length })
                    }
                    FileReadExtent::Unsatisfiable => Selection::Unsatisfiable(Default::default()),
                };
                Outcome::File(proto::FileReadMetadata {
                    total_size: metadata.total_size,
                    selection: Some(selection),
                    // An unrepresentable optional timestamp must not prevent reading the file.
                    modified_at: metadata.modified_at.and_then(|timestamp| {
                        proto_timestamp(
                            timestamp.timestamp(),
                            timestamp.timestamp_subsec_nanos() as i32,
                        )
                    }),
                })
            }
            FileReadHead::Absent => Outcome::Absent(Default::default()),
            FileReadHead::NotRegular => Outcome::NotRegular(Default::default()),
            FileReadHead::Symlink => Outcome::Symlink(Default::default()),
            FileReadHead::PermissionDenied => Outcome::PermissionDenied(Default::default()),
        };
        Self {
            outcome: Some(outcome),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn file_read_target_rejects_noncanonical_paths_before_normalization() {
        for path in [
            "", "relative", "/", "//", "/a/", "/a//b", "/./a", "/a/../b", "/a\\b", "/a\0b",
            "/a\nb", "/a\u{7f}", "/a\u{85}",
        ] {
            let target = FileReadTarget::Exact {
                file_path: path.into(),
            };
            assert_eq!(
                target.validate(),
                Err(InvalidFileReadTarget::InvalidPath),
                "{path:?}"
            );
            assert_eq!(
                FileReadTarget::try_from(proto::FileReadTarget::from(target)),
                Err(InvalidFileReadTarget::InvalidPath),
                "wire target {path:?}"
            );
        }
    }

    #[test]
    fn file_read_target_rejects_unsafe_suffix_components() {
        for segment in ["", ".", "..", "a/b", "a\\b", "a\0b", "a\nb", "a\u{7f}"] {
            let target = FileReadTarget::WithinRoot {
                root: "/public".into(),
                suffix: vec!["safe".into(), segment.into(), "leaf".into()],
                directory_request: false,
            };
            assert_eq!(
                target.validate(),
                Err(InvalidFileReadTarget::InvalidSuffix),
                "{segment:?}"
            );
            assert_eq!(
                FileReadTarget::try_from(proto::FileReadTarget::from(target)),
                Err(InvalidFileReadTarget::InvalidSuffix),
                "wire suffix {segment:?}"
            );
        }
    }

    #[test]
    fn file_read_target_preserves_filesystem_text_and_directory_intent() {
        for target in [
            FileReadTarget::Exact {
                file_path: "/public/%2e%2e/%2f/$1/café+e\u{301}".into(),
            },
            FileReadTarget::WithinRoot {
                root: "/".into(),
                suffix: vec![],
                directory_request: false,
            },
            FileReadTarget::WithinRoot {
                root: "/public".into(),
                suffix: vec![],
                directory_request: true,
            },
            FileReadTarget::WithinRoot {
                root: "/public/%2f".into(),
                suffix: vec!["%2e%2e".into(), "$1".into(), "a\u{85}b".into()],
                directory_request: true,
            },
        ] {
            target.validate().unwrap();
            assert_eq!(
                FileReadTarget::try_from(proto::FileReadTarget::from(target.clone())).unwrap(),
                target
            );
        }
    }

    #[test]
    fn file_read_target_rejects_invalid_root_and_missing_wire_target() {
        for root in ["public", "/public/..", "/public/", "/public//a", "/a\\b"] {
            let target = FileReadTarget::WithinRoot {
                root: root.into(),
                suffix: vec!["file".into()],
                directory_request: false,
            };
            assert_eq!(
                FileReadTarget::try_from(proto::FileReadTarget::from(target)),
                Err(InvalidFileReadTarget::InvalidPath),
                "{root:?}"
            );
        }
        assert_eq!(
            FileReadTarget::try_from(proto::FileReadTarget { target: None }),
            Err(InvalidFileReadTarget::MissingTarget)
        );
    }
}
