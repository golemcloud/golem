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

use crate::custom_api::PathSegment;
use rustc_hash::FxHashMap;

#[derive(Debug, Clone)]
pub struct RadixNode<T> {
    literals: FxHashMap<String, RadixNode<T>>,
    variable: Option<Box<RadixNode<T>>>,
    catch_all: Option<Box<RadixNode<T>>>,
    data: Option<T>,
}

impl<T> Default for RadixNode<T> {
    fn default() -> Self {
        Self {
            literals: FxHashMap::default(),
            variable: None,
            catch_all: None,
            data: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InsertionError {
    #[error("Conflict with existing route")]
    Conflict,
}

impl<T> RadixNode<T> {
    pub fn add_path(&mut self, path: &[PathSegment], data: T) -> Result<(), InsertionError> {
        let mut node = self;

        for (i, segment) in path.iter().enumerate() {
            match segment {
                PathSegment::Literal { value } => {
                    node = node.literals.entry(value.clone()).or_default();
                }
                PathSegment::Variable { .. } => {
                    let entry = node
                        .variable
                        .get_or_insert_with(|| Box::new(RadixNode::default()));
                    node = entry.as_mut();
                }
                PathSegment::CatchAll { .. } => {
                    if i != path.len() - 1 {
                        return Err(InsertionError::Conflict);
                    }
                    let entry = node
                        .catch_all
                        .get_or_insert_with(|| Box::new(RadixNode::default()));
                    node = entry.as_mut();
                    break;
                }
            }
        }

        if node.data.is_some() {
            Err(InsertionError::Conflict)
        } else {
            node.data = Some(data);
            Ok(())
        }
    }

    pub fn get_by_path(&self, path: &[PathSegment]) -> Option<&T> {
        let mut node = self;

        for segment in path {
            match segment {
                PathSegment::Literal { value } => {
                    node = node.literals.get(value)?;
                }
                PathSegment::Variable { .. } => {
                    node = node.variable.as_ref()?.as_ref();
                }
                PathSegment::CatchAll { .. } => {
                    node = node.catch_all.as_ref()?.as_ref();
                    break;
                }
            }
        }

        node.data.as_ref()
    }

    pub fn matches(&self, path: &[&str]) -> Option<(&T, Vec<String>)> {
        let mut pending = vec![(self, path, 0, None::<&[&str]>)];
        let mut bindings = Vec::new();
        while let Some((node, segments, capture_count, capture)) = pending.pop() {
            bindings.truncate(capture_count);
            if let Some(capture) = capture {
                bindings.push(capture.join("/"));
            }
            if segments.is_empty() {
                if let Some(data) = node.data.as_ref() {
                    return Some((data, bindings));
                }
                continue;
            }

            let seg = segments[0];
            let capture_count = bindings.len();
            // Push in reverse priority so incomplete literal/variable paths can backtrack.
            if let Some(child) = node.catch_all.as_deref() {
                pending.push((child, &[], capture_count, Some(segments)));
            }
            if let Some(child) = node.variable.as_deref() {
                pending.push((child, &segments[1..], capture_count, Some(&segments[..1])));
            }
            if let Some(child) = node.literals.get(seg) {
                pending.push((child, &segments[1..], capture_count, None));
            }
        }
        None
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use test_r::test;

    fn parse_segments(path: impl AsRef<str>) -> Vec<PathSegment> {
        split(path.as_ref()).map(parse_segment).collect()
    }

    fn matches_str<'a, T: 'a>(node: &'a RadixNode<T>, path: &str) -> Option<(&'a T, Vec<String>)> {
        let segments: Vec<&str> = split(path).collect();
        node.matches(&segments)
    }

    fn parse_segment(segment: &str) -> PathSegment {
        if segment == "*" {
            PathSegment::Variable {
                display_name: "unused".to_string(),
            }
        } else if segment == "**" {
            PathSegment::CatchAll {
                display_name: "unused".to_string(),
            }
        } else {
            PathSegment::Literal {
                value: segment.to_string(),
            }
        }
    }

    fn split(path: &str) -> impl Iterator<Item = &str> {
        path.trim_matches('/').split('/').filter(|s| !s.is_empty())
    }

    #[test]
    fn test_push_and_get() {
        let mut root = RadixNode::default();

        let path1 = parse_segments("/a/b/c");
        root.add_path(&path1, 1).unwrap();

        assert_eq!(matches_str(&root, "/a/b/c"), Some((&1, Vec::new())));

        let path2 = parse_segments("/a/b/d");
        root.add_path(&path2, 2).unwrap();

        assert_eq!(matches_str(&root, "/a/b/c"), Some((&1, Vec::new())));
        assert_eq!(matches_str(&root, "/a/b/d"), Some((&2, Vec::new())));
    }

    #[test]
    fn test_static_vs_variable_priority() {
        let mut root = RadixNode::default();

        root.add_path(&parse_segments("/components/worker"), 1)
            .unwrap();
        root.add_path(&parse_segments("/components/*"), 2).unwrap();

        assert_eq!(
            matches_str(&root, "/components/worker"),
            Some((&1, Vec::new()))
        );
        assert_eq!(
            matches_str(&root, "/components/123"),
            Some((&2, vec![String::from("123")]))
        );
    }

    #[test]
    fn test_multiple_variables() {
        let mut root = RadixNode::default();

        root.add_path(&parse_segments("/api/*/users/*"), 1).unwrap();

        assert_eq!(
            matches_str(&root, "/api/v1/users/123"),
            Some((&1, vec![String::from("v1"), String::from("123")]))
        );
    }

    #[test]
    fn test_catch_all() {
        let mut root = RadixNode::default();

        root.add_path(&parse_segments("/api/**"), 1).unwrap();

        assert_eq!(
            matches_str(&root, "/api/v1/users"),
            Some((&1, vec![String::from("v1/users")]))
        );
        assert_eq!(
            matches_str(&root, "/api/v2/users/123"),
            Some((&1, vec![String::from("v2/users/123")]))
        );
    }

    #[test]
    fn test_catch_all_fallthrough() {
        let mut root = RadixNode::default();

        root.add_path(&parse_segments("/api/**"), 1).unwrap();
        root.add_path(&parse_segments("/api/v1/**"), 2).unwrap();

        assert_eq!(
            matches_str(&root, "/api/v1/users"),
            Some((&2, vec![String::from("users")]))
        );
        assert_eq!(
            matches_str(&root, "/api/v2/users"),
            Some((&1, vec![String::from("v2/users")]))
        );
    }

    #[test]
    fn test_root_only() {
        let mut root = RadixNode::default();

        root.add_path(&parse_segments("/"), 1).unwrap();

        assert_eq!(matches_str(&root, "/"), Some((&1, Vec::new())));
    }

    #[test]
    fn backtracking_restores_captures_and_catchall_requires_one_segment() {
        let mut root = RadixNode::default();
        root.add_path(&parse_segments("/api/*/fixed/*/dead"), 1)
            .unwrap();
        root.add_path(&parse_segments("/api/*/*/last"), 2).unwrap();
        root.add_path(&parse_segments("/api/**"), 3).unwrap();
        assert_eq!(
            matches_str(&root, "/api/first/fixed/last"),
            Some((&2, vec!["first".into(), "fixed".into()]))
        );
        assert_eq!(
            matches_str(&root, "/api/first/fixed/x/miss"),
            Some((&3, vec!["first/fixed/x/miss".into()]))
        );
        assert_eq!(matches_str(&root, "/api"), None);
    }
}
