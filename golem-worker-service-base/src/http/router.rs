use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub struct RadixNode<T> {
    pattern: Vec<Pattern>,
    // The key is the first pattern of the child.
    // Given the paths are perfectly de-duplicated,
    // we can assume that each child has a unique first pattern.
    children: BTreeMap<Pattern, RadixNode<T>>,
    data: Option<T>,
}

impl<T> std::fmt::Debug for RadixNode<T>
where
    T: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RadixNode")
            .field("pattern", &self.pattern)
            .field("children", &self.children.values())
            .field("data", &self.data)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Pattern {
    Static(String),
    Variable,
}

#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error("Conflict with existing route")]
    Conflict,
}

impl<T> RadixNode<T> {
    pub fn insert(&mut self, path: &[Pattern], data: T) -> Result<(), PushError> {
        if path.is_empty() {
            if self.data.is_some() {
                return Err(PushError::Conflict);
            } else {
                self.data = Some(data);
                Ok(())
            }
        } else {
            let common_prefix_len = self.common_prefix_len(path);

            if common_prefix_len == self.pattern.len() {
                if common_prefix_len == path.len() {
                    // The path is fully consumed, update the data of the current node
                    if self.data.is_some() {
                        Err(PushError::Conflict)
                    } else {
                        self.data = Some(data);
                        Ok(())
                    }
                } else {
                    // There are remaining patterns in the path, insert into the appropriate child node
                    if let Some(child) = self.children.get_mut(&path[common_prefix_len]) {
                        child.insert(&path[common_prefix_len..], data)
                    } else {
                        self.insert_new(&path[common_prefix_len..], data);
                        Ok(())
                    }
                }
            } else {
                let new_child_pattern = self.pattern.split_off(common_prefix_len);
                let new_child_data = self.data.take();
                let new_child_children = std::mem::take(&mut self.children);

                let new_child = RadixNode {
                    pattern: new_child_pattern,
                    children: new_child_children,
                    data: new_child_data,
                };

                self.children
                    .insert(new_child.pattern[0].clone(), new_child);
                self.insert(path, data)
            }
        }
    }

    fn insert_new(&mut self, path: &[Pattern], data: T) {
        if self.pattern.is_empty() && self.data.is_none() {
            self.pattern = path.to_vec();
            self.data = Some(data);
        } else {
            let new_node = RadixNode {
                pattern: path.to_vec(),
                children: BTreeMap::new(),
                data: Some(data),
            };
            self.children.insert(path[0].clone(), new_node);
        }
    }

    pub fn get(&self, path: &[Pattern]) -> Option<&T> {
        if path.is_empty() {
            return self.data.as_ref();
        }

        let common_prefix_len = self.common_prefix_len(path);

        if common_prefix_len == self.pattern.len() {
            if common_prefix_len == path.len() {
                // The path fully matches the current node's pattern
                return self.data.as_ref();
            } else {
                // The path partially matches the current node's pattern
                if let Some(child) = self.children.get(&path[common_prefix_len]) {
                    return child.get(&path[common_prefix_len..]);
                }
            }
        }

        None
    }

    pub fn matches_str<'a, 'b>(&'a self, path: &'b str) -> Option<MatchResult<'a, 'b, T>> {
        // Purposefully not filtering out empty strings.
        let path = path.trim().trim_matches('/').split('/').collect::<Vec<_>>();

        self.matches(&path)
    }

    pub fn matches<'a, 'b>(&'a self, path: &[&'b str]) -> Option<MatchResult<'a, 'b, T>> {
        let mut node = self;
        let mut path_segments = path;
        let mut variables = Vec::new();

        loop {
            let common_prefix_len = node
                .pattern
                .iter()
                .zip(path_segments.iter())
                .take_while(|(a, b)| match a {
                    Pattern::Static(s) => s == *b,
                    Pattern::Variable => true,
                })
                .count();

            if common_prefix_len == node.pattern.len() {
                let path_start = path.len() - path_segments.len();
                let path_end = path_start + common_prefix_len;
                let new_variables = extract_values(&path[path_start..path_end], &node.pattern);
                variables.extend(new_variables);

                if common_prefix_len == path_segments.len() {
                    // The path fully matches the current node's pattern
                    return node
                        .data
                        .as_ref()
                        .map(|data| MatchResult { data, variables });
                } else {
                    // The path partially matches the current node's pattern
                    path_segments = &path_segments[common_prefix_len..];

                    match path_segments.first() {
                        Some(first_segment) => {
                            let next_child = node
                                .children
                                .iter()
                                .find(|(pattern, _)| match pattern {
                                    Pattern::Static(s) => s == first_segment,
                                    Pattern::Variable => true,
                                })
                                .map(|(_, child)| child);

                            if let Some(child) = next_child {
                                node = child;
                            } else {
                                break;
                            }
                        }
                        None => {
                            break;
                        }
                    }
                }
            } else {
                break;
            }
        }

        None
    }

    fn common_prefix_len(&self, path: &[Pattern]) -> usize {
        self.pattern
            .iter()
            .zip(path.iter())
            .take_while(|(a, b)| a == b)
            .count()
    }
}

fn extract_values<'input, 'slice>(
    paths: &'slice [&'input str],
    variables: &'slice [Pattern],
) -> impl Iterator<Item = &'input str> + 'slice {
    paths
        .iter()
        .zip(variables.iter())
        .filter_map(move |(path, variable)| match variable {
            Pattern::Variable => Some(*path),
            Pattern::Static(_) => None,
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MatchResult<'a, 'b, T> {
    pub data: &'a T,
    pub variables: Vec<&'b str>,
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_push_and_get() {
        let mut root = RadixNode::default();

        let path1 = vec![
            Pattern::Static("a".to_string()),
            Pattern::Static("b".to_string()),
            Pattern::Static("c".to_string()),
        ];

        root.insert(path1.as_slice(), 1).unwrap();
        assert_eq!(root.get(&path1), Some(&1),);

        // a/b/d
        let path2 = vec![
            Pattern::Static("a".to_string()),
            Pattern::Static("b".to_string()),
            Pattern::Static("d".to_string()),
        ];

        root.insert(path2.as_slice(), 2).unwrap();

        assert_eq!(root.get(&path1), Some(&1),);
        assert_eq!(root.get(&path2), Some(&2),);

        let path3 = vec![
            Pattern::Static("a".to_string()),
            Pattern::Static("a".to_string()),
        ];

        root.insert(path3.as_slice(), 3).unwrap();

        assert_eq!(root.get(&path1), Some(&1),);
        assert_eq!(root.get(&path2), Some(&2),);
        assert_eq!(root.get(&path3), Some(&3),);
    }

    #[test]
    fn test_push_subpath() {
        let mut root = RadixNode::default();

        let path1 = vec![
            Pattern::Static("a".to_string()),
            Pattern::Static("b".to_string()),
            Pattern::Static("c".to_string()),
        ];

        root.insert(path1.as_slice(), 1).unwrap();

        println!("{:#?}", root);

        let path2 = vec![
            Pattern::Static("a".to_string()),
            Pattern::Static("b".to_string()),
        ];

        root.insert(path2.as_slice(), 2).unwrap();

        println!("{:#?}", root);
    }

    #[test]
    fn test_conflict() {
        let mut root = RadixNode::default();

        let path = vec![
            Pattern::Static("a".to_string()),
            Pattern::Static("b".to_string()),
            Pattern::Static("c".to_string()),
        ];

        root.insert(path.as_slice(), 1).unwrap();

        assert!(matches!(
            root.insert(path.as_slice(), 2),
            Err(PushError::Conflict)
        ));
    }

    #[test]
    fn test_matches() {
        let mut root = RadixNode::default();

        let path1 = vec![
            Pattern::Static("templates".to_string()),
            Pattern::Variable,
            Pattern::Static("worker".to_string()),
        ];

        root.insert(path1.as_slice(), 1).unwrap();

        let result = root.matches_str("/templates/123/worker");

        assert_eq!(
            result,
            Some(MatchResult {
                data: &1,
                variables: vec!["123"]
            })
        );

        assert_eq!(root.matches_str("/templates/123/worker/extra"), None);
        assert_eq!(root.matches_str("/templates/123"), None);
    }

    #[test]
    fn test_matches_two_routes() {
        let mut root = RadixNode::default();

        let path1 = vec![
            Pattern::Static("templates".to_string()),
            Pattern::Variable,
            Pattern::Static("worker".to_string()),
        ];

        root.insert(path1.as_slice(), 1).unwrap();

        let path2 = vec![
            Pattern::Static("templates".to_string()),
            Pattern::Variable,
            Pattern::Static("function".to_string()),
        ];

        root.insert(path2.as_slice(), 2).unwrap();

        let result = root.matches_str("/templates/123/worker");

        assert_eq!(
            result,
            Some(MatchResult {
                data: &1,
                variables: vec!["123"]
            })
        );

        let result = root.matches_str("/templates/456/function");

        assert_eq!(
            result,
            Some(MatchResult {
                data: &2,
                variables: vec!["456"]
            })
        );
    }

    #[test]
    fn test_conflict_static_variable() {
        let mut root = RadixNode::default();

        let path1 = vec![
            Pattern::Static("templates".to_string()),
            Pattern::Static("worker".to_string()),
        ];

        root.insert(path1.as_slice(), 1).unwrap();

        let path2 = vec![Pattern::Static("templates".to_string()), Pattern::Variable];

        root.insert(path2.as_slice(), 2).unwrap();

        assert_eq!(
            Some(MatchResult {
                data: &1,
                variables: vec![]
            }),
            root.matches_str("/templates/worker")
        );

        assert_eq!(
            Some(MatchResult {
                data: &2,
                variables: vec!["123"]
            }),
            root.matches_str("/templates/123")
        );
    }

    #[test]
    fn test_multiple_variables() {
        let mut root = RadixNode::default();

        let path1 = vec![
            Pattern::Static("api".to_string()),
            Pattern::Variable,
            Pattern::Static("users".to_string()),
            Pattern::Variable,
        ];

        root.insert(path1.as_slice(), 1).unwrap();

        assert_eq!(
            Some(MatchResult {
                data: &1,
                variables: vec!["v1", "123"]
            }),
            root.matches_str("/api/v1/users/123")
        );
    }

    #[test]
    fn test_multiple_variables_different_order() {
        let mut root = RadixNode::default();

        let path1 = vec![
            Pattern::Static("api".to_string()),
            Pattern::Variable,
            Pattern::Static("users".to_string()),
            Pattern::Variable,
        ];

        root.insert(path1.as_slice(), 1).unwrap();

        let path2 = vec![
            Pattern::Static("api".to_string()),
            Pattern::Static("users".to_string()),
            Pattern::Variable,
            Pattern::Variable,
        ];

        root.insert(path2.as_slice(), 2).unwrap();

        assert_eq!(
            Some(MatchResult {
                data: &1,
                variables: vec!["v1", "123"]
            }),
            root.matches_str("/api/v1/users/123")
        );

        assert_eq!(
            Some(MatchResult {
                data: &2,
                variables: vec!["456", "789"]
            }),
            root.matches_str("/api/users/456/789")
        );
    }

    #[test]
    fn test_conflict_variable_static() {
        let mut root = RadixNode::default();

        let path1 = vec![
            Pattern::Static("api".to_string()),
            Pattern::Variable,
            Pattern::Static("users".to_string()),
        ];

        root.insert(path1.as_slice(), 1).unwrap();

        let path2 = vec![
            Pattern::Static("api".to_string()),
            Pattern::Static("v1".to_string()),
            Pattern::Static("users".to_string()),
        ];

        root.insert(path2.as_slice(), 2).unwrap();

        assert_eq!(
            Some(MatchResult {
                data: &2,
                variables: vec![]
            }),
            root.matches_str("/api/v1/users")
        );

        assert_eq!(
            Some(MatchResult {
                data: &1,
                variables: vec!["v2"]
            }),
            root.matches_str("/api/v2/users")
        );
    }

    #[test]
    fn test_multiple_routes_resolution() {
        let mut root = RadixNode::default();

        let path1 = vec![
            Pattern::Static("api".to_string()),
            Pattern::Variable,
            Pattern::Static("users".to_string()),
        ];

        root.insert(path1.as_slice(), 1).unwrap();

        let path2 = vec![
            Pattern::Static("api".to_string()),
            Pattern::Static("v1".to_string()),
            Pattern::Static("users".to_string()),
            Pattern::Variable,
        ];

        root.insert(path2.as_slice(), 2).unwrap();

        let path3 = vec![
            Pattern::Static("api".to_string()),
            Pattern::Variable,
            Pattern::Static("users".to_string()),
            Pattern::Static("profile".to_string()),
        ];

        root.insert(path3.as_slice(), 3).unwrap();

        println!("{:#?}", root);

        assert_eq!(
            Some(MatchResult {
                data: &1,
                variables: vec!["v2"]
            }),
            root.matches_str("/api/v2/users")
        );

        assert_eq!(
            Some(MatchResult {
                data: &2,
                variables: vec!["123"]
            }),
            root.matches_str("/api/v1/users/123")
        );

        assert_eq!(
            Some(MatchResult {
                data: &3,
                variables: vec!["456"]
            }),
            root.matches_str("/api/456/users/profile")
        );
    }
}
