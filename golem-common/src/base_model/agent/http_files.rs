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
use std::collections::HashSet;

/// A public request target with one shared segment boundary for routing and files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequestTarget {
    path: String,
    query: Option<String>,
    segments: Vec<String>,
    trailing_slash: bool,
}

impl HttpRequestTarget {
    pub fn parse(target: &str) -> Result<Self, String> {
        let (path, query) = match target.split_once('?') {
            Some((path, query)) => (path, Some(query.to_string())),
            None => (target, None),
        };
        let remaining = path.strip_prefix('/').ok_or("unsafe-path")?;
        let trailing_slash = !remaining.is_empty() && remaining.ends_with('/');
        let segments = if remaining.is_empty() {
            Vec::new()
        } else {
            let remaining = if trailing_slash {
                &remaining[..remaining.len() - 1]
            } else {
                remaining
            };
            remaining
                .split('/')
                .map(decode_segment)
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(Self {
            path: path.to_string(),
            query,
            segments,
            trailing_slash,
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    pub fn trailing_slash(&self) -> bool {
        self.trailing_slash
    }
}

impl FileMapping {
    /// Compile URI source syntax once; targets are filesystem text, not URIs.
    pub fn compile(source: &str, target: &str) -> Result<Self, String> {
        let (source, subtree) = match source.strip_suffix("/*") {
            Some(prefix) if prefix.ends_with('/') => return Err("source-path".into()),
            Some(prefix) => (if prefix.is_empty() { "/" } else { prefix }, true),
            None => (source, false),
        };
        if source.contains('*') {
            return Err("source-wildcard".into());
        }
        let segments = decode_source(source)?;
        let mapping = if subtree {
            let root = target.strip_suffix("/$1").ok_or("target-placeholder")?;
            if root.ends_with('/') {
                return Err("target-path".into());
            }
            Self::Subtree(SubtreeFileMapping {
                public_prefix: segments,
                filesystem_root: if root.is_empty() {
                    "/".into()
                } else {
                    root.into()
                },
            })
        } else {
            Self::Exact(ExactFileMapping {
                public_path: segments,
                file_path: target.into(),
            })
        };
        mapping.validate()?;
        Ok(mapping)
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
        if segments
            .iter()
            .any(|segment| !valid_decoded_segment(segment))
        {
            return Err("source-path".into());
        }
        if target.contains('$') {
            return Err("target-placeholder".into());
        }
        if !target.starts_with('/')
            || target.chars().any(char::is_control)
            || (!allow_root && target == "/")
            || (target != "/"
                && target[1..]
                    .split('/')
                    .any(|segment| !valid_decoded_segment(segment)))
        {
            return Err("target-path".into());
        }
        Ok(())
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

pub fn valid_decoded_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment
            .bytes()
            .any(|byte| byte <= 0x1f || byte == 0x7f || byte == b'/' || byte == b'\\')
}

fn decode_source(source: &str) -> Result<Vec<String>, String> {
    if source.contains(['$', '*', '?', '#']) {
        return Err("source-path".into());
    }
    let target = HttpRequestTarget::parse(source).map_err(|_| "source-path")?;
    if target.trailing_slash {
        return Err("source-path".into());
    }
    Ok(target.segments)
}

fn decode_segment(raw: &str) -> Result<String, String> {
    let mut decoded = Vec::with_capacity(raw.len());
    let mut bytes = raw.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = bytes.next().and_then(hex).ok_or("unsafe-path")?;
            let low = bytes.next().and_then(hex).ok_or("unsafe-path")?;
            decoded.push(high * 16 + low);
        } else if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@".contains(&byte) {
            decoded.push(byte);
        } else {
            return Err("unsafe-path".into());
        }
    }
    let decoded = String::from_utf8(decoded).map_err(|_| "unsafe-path")?;
    if !valid_decoded_segment(&decoded) {
        return Err("unsafe-path".into());
    }
    Ok(decoded)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use test_r::test;

    #[test]
    fn shared_request_path_corpus() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
        ))
        .unwrap();
        for case in corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["suite"] == "path")
        {
            let result = HttpRequestTarget::parse(case["input"]["target"].as_str().unwrap());
            if let Some(error) = case["expect"]["error"].as_str() {
                assert_eq!(result.unwrap_err(), error, "{}", case["id"]);
            } else {
                let result = result.unwrap();
                assert_eq!(
                    json!({
                        "segments": result.segments(),
                        "trailing_slash": result.trailing_slash(),
                        "path": result.path(),
                        "query": result.query(),
                    }),
                    case["expect"],
                    "{}",
                    case["id"]
                );
            }
        }
    }

    #[test]
    fn request_literals_do_not_acquire_mapping_syntax() {
        let target = HttpRequestTarget::parse("/$1/*/%7Bid%7D/%252f?x=??%ff").unwrap();
        assert_eq!(target.segments(), &["$1", "*", "{id}", "%2f"]);
        assert_eq!(target.query(), Some("x=??%ff"));
        assert!(FileMapping::compile("/$1", "/file").is_err());
        assert!(FileMapping::compile("/a*b", "/file").is_err());
        assert!(FileMapping::compile("/{id}", "/file").is_err());
    }

    #[test]
    fn shared_mapping_corpus() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
        ))
        .unwrap();
        for case in corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["suite"] == "mapping")
        {
            let result = FileMapping::compile_list(
                case["input"]["mappings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|pair| (pair[0].as_str().unwrap(), pair[1].as_str().unwrap())),
            );
            if let Some(error) = case["expect"]["error"].as_str() {
                assert_eq!(result.unwrap_err(), error, "{}", case["id"]);
            } else {
                let actual: Vec<Value> = result.unwrap().into_iter().map(|mapping| match mapping {
                    FileMapping::Exact(mapping) => json!({"Exact": {"public_path": mapping.public_path, "file_path": mapping.file_path}}),
                    FileMapping::Subtree(mapping) => json!({"Subtree": {"public_prefix": mapping.public_prefix, "filesystem_root": mapping.filesystem_root}}),
                }).collect();
                assert_eq!(json!(actual), case["expect"]["compiled"], "{}", case["id"]);
            }
        }
    }

    #[test]
    fn raw_grammar_precedes_single_decoding() {
        for (source, segment) in [
            ("/%252e%252e", "%2e%2e"),
            ("/%252f", "%2f"),
            ("/%2A", "*"),
            ("/%241", "$1"),
            ("/%7Bid%7D", "{id}"),
            ("/a+b", "a+b"),
            ("/%C3%A9", "é"),
        ] {
            let expected = FileMapping::Exact(ExactFileMapping {
                public_path: vec![segment.into()],
                file_path: "/%2e%2e".into(),
            });
            assert_eq!(FileMapping::compile(source, "/%2e%2e").unwrap(), expected);
            expected.validate().unwrap();
        }
        for source in [
            "", "relative", "//", "/x/", "/x//y", "/.", "/%2e.", "/%2f", "/%5c", "/%00", "/%1f",
            "/%7f", "/%ff", "/%C0%AF", "/%", "/%0", "/%gg", "/a b", "/a#b", "/a?b", "/{id}", "/$1",
            "/é",
        ] {
            assert!(FileMapping::compile(source, "/file").is_err(), "{source}");
        }
    }

    #[test]
    fn structural_paths_are_checked_without_decoding() {
        for segment in ["", ".", "..", "a/b", "a\\b", "a\0b", "\u{7f}"] {
            let mapping = FileMapping::Exact(ExactFileMapping {
                public_path: vec![segment.into()],
                file_path: "/file".into(),
            });
            assert!(mapping.validate().is_err(), "{segment:?}");
        }
        for target in [
            "", "file", "/", "//file", "/file/", "/a//b", "/a/../b", "/a/./b", "/a\\b", "/a\0b",
            "/a\u{7f}", "/$1", "/$2",
        ] {
            assert!(FileMapping::compile("/file", target).is_err(), "{target:?}");
        }
        assert!(FileMapping::compile("/*", "/$1").is_ok());
        assert!(FileMapping::compile("//*", "/$1").is_err());
        assert!(FileMapping::compile("/*", "//$1").is_err());
        assert!(FileMapping::compile("/*", "/a/$1/$1").is_err());
        assert!(FileMapping::compile("/*", "/a//$1").is_err());
        assert!(FileMapping::compile("/file", "/é %20name").is_ok());
    }

    #[test]
    fn filesystem_controls_are_stricter_than_public_segments() {
        assert!(FileMapping::compile("/file", "/a\u{85}b").is_err());
        assert!(FileMapping::compile("/*", "/a\u{85}b/$1").is_err());
        assert!(FileMapping::compile("/a%C2%85b", "/file").is_ok());
        assert!(FileMapping::compile("/file", "/a\u{a0}b").is_ok());
    }
}
