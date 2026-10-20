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

use super::{ExactFileMapping, FileMapping, SubtreeFileMapping};
pub use golem_schema::http::{HttpRequestTarget, valid_decoded_segment};
use std::collections::HashSet;

impl FileMapping {
    /// Compile URI source syntax once; targets are filesystem text, not URIs.
    pub fn compile(source: &str, target: &str) -> Result<Self, String> {
        Ok(
            match golem_schema::http::FileMapping::compile(source, target)? {
                golem_schema::http::FileMapping::Exact {
                    public_path,
                    file_path,
                } => Self::Exact(ExactFileMapping {
                    public_path,
                    file_path,
                }),
                golem_schema::http::FileMapping::Subtree {
                    public_prefix,
                    filesystem_root,
                } => Self::Subtree(SubtreeFileMapping {
                    public_prefix,
                    filesystem_root,
                }),
            },
        )
    }

    pub fn compile_list<'a>(
        mappings: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Vec<Self>, String> {
        let compiled = mappings
            .into_iter()
            .map(|(source, target)| Self::compile(source, target))
            .collect::<Result<Vec<_>, _>>()?;
        Self::validate_list(&compiled)?;
        Ok(compiled)
    }

    /// Structural metadata already contains decoded public segments.
    pub fn validate(&self) -> Result<(), String> {
        let (segments, target, allow_root) = match self {
            Self::Exact(mapping) => (&mapping.public_path, mapping.file_path.as_str(), false),
            Self::Subtree(mapping) => (
                &mapping.public_prefix,
                mapping.filesystem_root.as_str(),
                true,
            ),
        };
        golem_schema::http::validate_file_mapping(segments, target, allow_root)
    }

    pub fn validate_list(mappings: &[Self]) -> Result<(), String> {
        let mut seen = HashSet::new();
        for mapping in mappings {
            mapping.validate()?;
            if !seen.insert(mapping) {
                return Err("duplicate-mapping".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
