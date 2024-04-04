#[derive(Clone, Default)]
pub struct RadixNode<T> {
    pattern: Vec<Pattern>,
    children: OrderedChildren<T>,
    data: Option<T>,
}

impl<T> std::fmt::Debug for RadixNode<T>
where
    T: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RadixNode")
            .field("pattern", &self.pattern)
            .field("children", &self.children)
            .field("data", &self.data)
            .finish()
    }
}

// Given the paths are perfectly de-duplicated,
// We can assume that each child has a unique first pattern.
#[derive(Debug, Clone)]
struct OrderedChildren<T>(Vec<RadixNode<T>>);

impl<T> Default for OrderedChildren<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T> OrderedChildren<T> {
    #[inline]
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[inline]
    fn search_by_str(&self, input: &str) -> Option<&RadixNode<T>> {
        let next_child_index = self.0.binary_search_by(|child| match &child.pattern[0] {
            Pattern::Static(s) => s.as_str().cmp(input),
            Pattern::Variable => std::cmp::Ordering::Equal,
        });

        next_child_index.ok().map(|index| &self.0[index])
    }

    fn get_child(&self, pattern: &Pattern) -> Option<&RadixNode<T>> {
        let index = self
            .0
            .binary_search_by(|c| c.pattern.first().cmp(&Some(pattern)))
            .ok()?;
        self.0.get(index)
    }

    fn get_child_mut(&mut self, pattern: &Pattern) -> Option<&mut RadixNode<T>> {
        let index = self
            .0
            .binary_search_by(|c| c.pattern.first().cmp(&Some(pattern)))
            .ok()?;
        self.0.get_mut(index)
    }

    fn add_child(&mut self, node: RadixNode<T>) {
        let index = self
            .0
            .binary_search_by(|c| c.pattern.cmp(&node.pattern))
            .unwrap_or_else(|i| i);
        self.0.insert(index, node);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Pattern {
    Static(String),
    Variable,
}

#[derive(Debug, thiserror::Error)]
pub enum InsertionError {
    #[error("Conflict with existing route")]
    Conflict,
}

impl<T> RadixNode<T> {
    pub fn insert_path(&mut self, path: &[Pattern], data: T) -> Result<(), InsertionError> {
        if path.is_empty() {
            if self.data.is_some() {
                Err(InsertionError::Conflict)
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
                        Err(InsertionError::Conflict)
                    } else {
                        self.data = Some(data);
                        Ok(())
                    }
                } else {
                    let new_path = &path[common_prefix_len..];

                    if let Some(child) = self.children.get_child_mut(&new_path[0]) {
                        child.insert_path(new_path, data)
                    } else {
                        self.insert_new(new_path, data)
                    }
                }
            } else if common_prefix_len == 0 {
                // both self and path must become children of the current node.
                //
                // self.path = ["a", "b"]
                // self.data = Some(1)
                //
                // path = ["c", "d"]
                // data = Some(2)
                //
                // becomes
                //
                // self.path = []
                // self.data = None
                // self.children = {
                //     "a" => RadixNode { path = ["a, "b"], data = Some(1)}
                //     "c" => RadixNode { path = ["c", "d"], data = Some(2)}
                // }

                let self_node = RadixNode {
                    pattern: std::mem::take(&mut self.pattern),
                    children: std::mem::take(&mut self.children),
                    data: self.data.take(),
                };

                let path_node = RadixNode {
                    pattern: path.to_vec(),
                    children: OrderedChildren::default(),
                    data: Some(data),
                };

                self.children.add_child(self_node);
                self.children.add_child(path_node);

                Ok(())
            } else {
                // The path partially matches the current node's pattern
                //
                // self.path = ["a", "b"]
                // self.data = Some(1)
                // self.children = {
                //     "c" => RadixNode { path = ["c"], data = Some(2)}
                // }
                //
                // path = ["a", "c"]
                // data = Some(3)
                //
                // becomes
                //
                // self.path = ["a"]
                // self.data = None
                // self.children = {
                //     "b" => RadixNode {
                //               path = ["b"],
                //               data = Some(1),
                //               children = {
                //                  RadixNode { path = ["c"], data = Some(2)}
                //               }
                //            }
                //     "c" => RadixNode { path = ["c"], data = Some(3)}
                // }
                //
                // NOTE: The C gets inserted in the recursive call.
                // This iteration will only create the "b" node by splitting the current node.

                let new_child_pattern = self.pattern.split_off(common_prefix_len);
                let new_child_data = self.data.take();
                let new_child_children = std::mem::take(&mut self.children);

                let new_child = RadixNode {
                    pattern: new_child_pattern,
                    children: new_child_children,
                    data: new_child_data,
                };

                self.children.add_child(new_child);
                self.insert_path(path, data)
            }
        }
    }

    pub fn get(&self, path: &[Pattern]) -> Option<&T> {
        if path.is_empty() {
            return self.data.as_ref();
        }

        let common_prefix_len = self.common_prefix_len(path);

        // Must match the entire pattern
        if common_prefix_len == self.pattern.len() {
            if common_prefix_len == path.len() {
                // The path fully matches the current node's pattern
                return self.data.as_ref();
            } else {
                // The path partially matches the current node's pattern
                if let Some(child) = self.children.get_child(&path[common_prefix_len]) {
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

                let new_variables = path[path_start..path_end]
                    .iter()
                    .zip(&node.pattern)
                    .filter_map(|(path, pattern)| match pattern {
                        Pattern::Variable => Some(*path),
                        Pattern::Static(_) => None,
                    });
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
                            let next_child = self.children.search_by_str(first_segment);
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

    fn insert_new(&mut self, path: &[Pattern], data: T) -> Result<(), InsertionError> {
        if self.children.is_empty() && self.pattern.is_empty() {
            if self.data.is_some() {
                Err(InsertionError::Conflict)
            } else {
                self.pattern = path.to_vec();
                self.data = Some(data);
                Ok(())
            }
        } else {
            let new_node = RadixNode {
                pattern: path.to_vec(),
                children: OrderedChildren::default(),
                data: Some(data),
            };
            self.children.add_child(new_node);
            Ok(())
        }
    }

    #[inline]
    fn common_prefix_len(&self, path: &[Pattern]) -> usize {
        self.pattern
            .iter()
            .zip(path.iter())
            .take_while(|(a, b)| a == b)
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MatchResult<'a, 'b, T> {
    pub data: &'a T,
    pub variables: Vec<&'b str>,
}

// Is pub so that it can be used in benchmark.
pub fn make_path(path: &str) -> Vec<Pattern> {
    path.trim_matches('/')
        .split('/')
        .map(|s| {
            if s.starts_with(':') {
                Pattern::Variable
            } else {
                Pattern::Static(s.to_string())
            }
        })
        .collect()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_push_and_get() {
        let mut root = RadixNode::default();

        let path1 = make_path("/a/b/c");
        root.insert_path(path1.as_slice(), 1).unwrap();

        assert_eq!(root.get(&path1), Some(&1));

        let path2 = make_path("/a/b/d");
        root.insert_path(path2.as_slice(), 2).unwrap();

        assert_eq!(root.get(&path1), Some(&1));
        assert_eq!(root.get(&path2), Some(&2));

        let path3 = make_path("/a/b/e");
        root.insert_path(path3.as_slice(), 3).unwrap();

        assert_eq!(root.get(&path1), Some(&1));
        assert_eq!(root.get(&path2), Some(&2));
        assert_eq!(root.get(&path3), Some(&3));
    }

    #[test]
    fn test_shape_no_overlap() {
        let mut root = RadixNode::default();

        let path1 = make_path("/a/b");
        root.insert_path(path1.as_slice(), 1).unwrap();

        let path2 = make_path("/d/e");
        root.insert_path(path2.as_slice(), 2).unwrap();

        let path3 = make_path("/f/g");
        root.insert_path(path3.as_slice(), 3).unwrap();

        assert_eq!(root.get(&path1), Some(&1));
        assert_eq!(root.get(&path2), Some(&2));

        assert!(root.pattern.is_empty());
        assert_eq!(3, root.children.0.len());
    }

    #[test]
    fn test_large_tree_structure() {
        let paths = [
            "/activity",
            "/suggestions",
            "/feed/trending",
            "/suggestions/tags",
            "/analytics/users",
            "/api/v2/users",
            "/api/v2/users/:user_id",
            "/dashboard/analytics",
            "/posts/:post_id/comments/:comment_id/replies",
            "/posts/:post_id/comments",
            "/trending/posts",
        ];

        let mut root = RadixNode::default();

        for (index, path) in paths.iter().enumerate() {
            let path = make_path(path);
            root.insert_path(&path, index).unwrap();
        }

        assert!(root.matches_str("/activity").is_some());
        assert!(root.matches_str("/suggestions").is_some());
        assert!(root.matches_str("/feed/trending").is_some());
        assert!(root.matches_str("/suggestions/tags").is_some());
        assert!(root.matches_str("/analytics/users").is_some());
        assert!(root.matches_str("/api/v2/users").is_some());
        assert!(root.matches_str("/api/v2/users/nico").is_some());
        assert!(root.matches_str("/dashboard/analytics").is_some());
        assert!(root
            .matches_str("/posts/123/comments/123/replies")
            .is_some());
        assert!(root.matches_str("/posts/123/comments").is_some());
        assert!(root.matches_str("/trending/posts").is_some());
    }

    #[test]
    fn test_push_subpath() {
        let mut root = RadixNode::default();

        let path1 = vec![
            Pattern::Static("a".to_string()),
            Pattern::Static("b".to_string()),
            Pattern::Static("c".to_string()),
        ];

        root.insert_path(path1.as_slice(), 1).unwrap();

        let path2 = vec![
            Pattern::Static("a".to_string()),
            Pattern::Static("b".to_string()),
        ];

        root.insert_path(path2.as_slice(), 2).unwrap();
    }

    #[test]
    fn test_conflict() {
        let mut root = RadixNode::default();

        let path = make_path("/a/b/c");

        root.insert_path(path.as_slice(), 1).unwrap();

        assert!(matches!(
            root.insert_path(path.as_slice(), 2),
            Err(InsertionError::Conflict)
        ));
    }

    #[test]
    fn test_matches() {
        let mut root = RadixNode::default();

        let path1 = make_path("/templates/:id/worker");

        root.insert_path(path1.as_slice(), 1).unwrap();

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

        let path1 = make_path("/templates/:id/worker");

        root.insert_path(path1.as_slice(), 1).unwrap();

        let path2 = make_path("/templates/:id/function");

        root.insert_path(path2.as_slice(), 2).unwrap();

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

        let path1 = make_path("/templates/worker");

        root.insert_path(path1.as_slice(), 1).unwrap();

        let path2 = make_path("/templates/:id");

        root.insert_path(path2.as_slice(), 2).unwrap();

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

        let path1 = make_path("/api/:version/users/:id");

        root.insert_path(path1.as_slice(), 1).unwrap();

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

        let path1 = make_path("/api/:api_id/users/:user_id");

        root.insert_path(path1.as_slice(), 1).unwrap();

        let path2 = make_path("/api/users/:user_id/:id");

        root.insert_path(path2.as_slice(), 2).unwrap();

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

        let path1 = make_path("/api/:version/users");

        root.insert_path(path1.as_slice(), 1).unwrap();

        let path2 = make_path("/api/v1/users");

        root.insert_path(path2.as_slice(), 2).unwrap();

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

        let path1 = make_path("/api/:version/users");
        root.insert_path(path1.as_slice(), 1).unwrap();

        let path2 = make_path("/api/v1/users/:id");
        root.insert_path(path2.as_slice(), 2).unwrap();

        let path3 = make_path("/api/:api_id/users/profile");
        root.insert_path(path3.as_slice(), 3).unwrap();

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
