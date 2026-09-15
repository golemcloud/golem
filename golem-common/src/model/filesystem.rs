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
use golem_api_grpc::proto::golem::worker as proto;

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
