use std::collections::BTreeMap;

#[derive(Clone, Default)]
struct RadixNode<T> {
    pattern: Vec<Pattern>,
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
enum Pattern {
    Static(String),
    Variable,
}

impl<T: std::fmt::Debug> RadixNode<T> {
    pub fn new(pattern: Vec<Pattern>) -> Self {
        Self {
            pattern,
            children: BTreeMap::new(),
            data: None,
        }
    }

    pub fn push(&mut self, path: &[Pattern], data: T) {
        if path.is_empty() {
            self.data = Some(data);
            return;
        }

        if let Some(child) = self.children.get_mut(&path[0]) {
            let common_prefix_len = child
                .pattern
                .iter()
                .zip(path.iter())
                .take_while(|(a, b)| a == b)
                .count();

            if common_prefix_len > 0 {
                if common_prefix_len == child.pattern.len() {
                    child.push(&path[common_prefix_len..], data);
                } else {
                    let new_child_pattern = child.pattern.split_off(common_prefix_len);
                    let new_child_data = child.data.take();
                    let new_child_children = std::mem::take(&mut child.children);

                    let new_child = RadixNode {
                        pattern: new_child_pattern,
                        children: new_child_children,
                        data: new_child_data,
                    };

                    child
                        .children
                        .insert(new_child.pattern[0].clone(), new_child);
                    child.push(&path[common_prefix_len..], data);
                }
                return;
            }
        }

        let new_node = RadixNode::new(path.to_vec());
        self.children.insert(path[0].clone(), new_node);
        if let Some(child) = self.children.get_mut(&path[0]) {
            child.data = Some(data);
        }
    }

    pub fn get(&self, path: &[Pattern]) -> Option<&T> {
        if path.is_empty() {
            return self.data.as_ref();
        }

        if let Some(child) = self.children.get(&path[0]) {
            if path.starts_with(&child.pattern) {
                return child.get(&path[child.pattern.len()..]);
            }
        }

        None
    }

    pub fn matches<'a, 'b>(&'a self, path: &'b str) -> Option<MatchResult<'a, 'b, T>> {
        let path = path
            .trim()
            .trim_start_matches('/')
            .trim_end_matches('/')
            .split('/')
            .collect::<Vec<_>>();

        println!("path: {path:?}");

        let mut node = self;
        let mut path_segments = path.as_slice();
        let mut variables = Vec::new();

        loop {
            if path_segments.is_empty() {
                let new_values = extract_values(path_segments, &node.pattern);
                variables.extend(new_values);
                return node
                    .data
                    .as_ref()
                    .map(|data| MatchResult { data, variables });
            }

            let current_segment = path_segments[0];

            println!("current_segment: {current_segment:?}");

            let next_child = node
                .children
                .iter()
                .find(|(pattern, _)| match pattern {
                    Pattern::Static(s) => s == current_segment,
                    Pattern::Variable => true,
                })
                .map(|(_, child)| child);

            if let Some(child) = next_child {
                println!("child: {child:#?}");

                let common_prefix_len = child
                    .pattern
                    .iter()
                    .zip(path_segments.iter())
                    .take_while(|(a, b)| match a {
                        Pattern::Static(s) => s == *b,
                        Pattern::Variable => true,
                    })
                    .count();

                if common_prefix_len == child.pattern.len() {
                    let path_start = path.len() - path_segments.len();
                    let path_end = path_start + common_prefix_len;

                    let new_variables = extract_values(&path[path_start..path_end], &child.pattern);
                    variables.extend(new_variables);

                    path_segments = &path_segments[common_prefix_len..];
                    node = child;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        None
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

        root.push(path1.as_slice(), 1);
        assert_eq!(root.get(&path1), Some(&1),);

        let path2 = vec![
            Pattern::Static("a".to_string()),
            Pattern::Static("b".to_string()),
            Pattern::Static("d".to_string()),
        ];

        root.push(path2.as_slice(), 2);

        assert_eq!(root.get(&path1), Some(&1),);
        assert_eq!(root.get(&path2), Some(&2),);

        let path3 = vec![
            Pattern::Static("a".to_string()),
            Pattern::Static("a".to_string()),
        ];

        root.push(path3.as_slice(), 3);

        assert_eq!(root.get(&path1), Some(&1),);
        assert_eq!(root.get(&path2), Some(&2),);
        assert_eq!(root.get(&path3), Some(&3),);
    }

    #[test]
    fn test_matches() {
        let mut root = RadixNode::default();

        let path1 = vec![
            Pattern::Static("templates".to_string()),
            Pattern::Variable,
            Pattern::Static("worker".to_string()),
        ];

        root.push(path1.as_slice(), 1);

        let result = root.matches("/templates/123/worker");

        assert_eq!(
            result,
            Some(MatchResult {
                data: &1,
                variables: vec!["123"]
            })
        );

        assert_eq!(root.matches("/templates/123/worker/extra"), None);
        assert_eq!(root.matches("/templates/123"), None);
    }

    #[test]
    fn test_matches_two_routes() {
        let mut root = RadixNode::default();

        let path1 = vec![
            Pattern::Static("templates".to_string()),
            Pattern::Variable,
            Pattern::Static("worker".to_string()),
        ];

        root.push(path1.as_slice(), 1);

        let path2 = vec![
            Pattern::Static("templates".to_string()),
            Pattern::Variable,
            Pattern::Static("function".to_string()),
        ];

        root.push(path2.as_slice(), 2);

        let result = root.matches("/templates/123/worker");

        assert_eq!(
            result,
            Some(MatchResult {
                data: &1,
                variables: vec!["123"]
            })
        );

        let result = root.matches("/templates/456/function");

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

        root.push(path1.as_slice(), 1);

        let path2 = vec![Pattern::Static("templates".to_string()), Pattern::Variable];

        root.push(path2.as_slice(), 2);

        assert_eq!(
            Some(MatchResult {
                data: &1,
                variables: vec![]
            }),
            root.matches("/templates/worker")
        );

        assert_eq!(
            Some(MatchResult {
                data: &2,
                variables: vec!["123"]
            }),
            root.matches("/templates/123")
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

        root.push(path1.as_slice(), 1);

        assert_eq!(
            Some(MatchResult {
                data: &1,
                variables: vec!["v1", "123"]
            }),
            root.matches("/api/v1/users/123")
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

        root.push(path1.as_slice(), 1);

        let path2 = vec![
            Pattern::Static("api".to_string()),
            Pattern::Static("users".to_string()),
            Pattern::Variable,
            Pattern::Variable,
        ];

        root.push(path2.as_slice(), 2);

        assert_eq!(
            Some(MatchResult {
                data: &1,
                variables: vec!["v1", "123"]
            }),
            root.matches("/api/v1/users/123")
        );

        assert_eq!(
            Some(MatchResult {
                data: &2,
                variables: vec!["456", "789"]
            }),
            root.matches("/api/users/456/789")
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

        root.push(path1.as_slice(), 1);

        let path2 = vec![
            Pattern::Static("api".to_string()),
            Pattern::Static("v1".to_string()),
            Pattern::Static("users".to_string()),
        ];

        root.push(path2.as_slice(), 2);

        assert_eq!(
            Some(MatchResult {
                data: &2,
                variables: vec![]
            }),
            root.matches("/api/v1/users")
        );

        assert_eq!(
            Some(MatchResult {
                data: &1,
                variables: vec!["v2"]
            }),
            root.matches("/api/v2/users")
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

        root.push(path1.as_slice(), 1);

        let path2 = vec![
            Pattern::Static("api".to_string()),
            Pattern::Static("v1".to_string()),
            Pattern::Static("users".to_string()),
            Pattern::Variable,
        ];

        root.push(path2.as_slice(), 2);

        let path3 = vec![
            Pattern::Static("api".to_string()),
            Pattern::Variable,
            Pattern::Static("users".to_string()),
            Pattern::Static("profile".to_string()),
        ];

        root.push(path3.as_slice(), 3);

        assert_eq!(
            Some(MatchResult {
                data: &1,
                variables: vec!["v2"]
            }),
            root.matches("/api/v2/users")
        );

        assert_eq!(
            Some(MatchResult {
                data: &2,
                variables: vec!["123"]
            }),
            root.matches("/api/v1/users/123")
        );

        assert_eq!(
            Some(MatchResult {
                data: &3,
                variables: vec!["456"]
            }),
            root.matches("/api/456/users/profile")
        );
    }
}
