use std::collections::HashMap;

#[derive(Clone, Default)]
struct RadixNode<T> {
    pattern: Vec<Pattern>,
    children: HashMap<Pattern, RadixNode<T>>,
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
    Variable(String),
}

impl<T> RadixNode<T> {
    pub fn new(pattern: Vec<Pattern>) -> Self {
        Self {
            pattern,
            children: HashMap::new(),
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
}

#[test]
fn test_push_and_get() {
    let mut root = RadixNode::default();

    let path1 = vec![
        Pattern::Static("a".to_string()),
        Pattern::Static("b".to_string()),
        Pattern::Static("c".to_string()),
    ];

    root.push(path1.as_slice(), 1);
    println!("{:#?}", root);
    assert_eq!(root.get(&path1), Some(&1),);

    let path2 = vec![
        Pattern::Static("a".to_string()),
        Pattern::Static("b".to_string()),
        Pattern::Static("d".to_string()),
    ];

    root.push(path2.as_slice(), 2);
    println!("{:#?}", root);

    assert_eq!(root.get(&path1), Some(&1),);
    assert_eq!(root.get(&path2), Some(&2),);

    let path3 = vec![
        Pattern::Static("a".to_string()),
        Pattern::Static("a".to_string()),
    ];

    root.push(path3.as_slice(), 3);
    println!("{:#?}", root);

    assert_eq!(root.get(&path3), Some(&3),);
}
