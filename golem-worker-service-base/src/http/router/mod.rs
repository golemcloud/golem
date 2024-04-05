use std::collections::HashMap;

use hyper::http::Method;

use crate::api_definition::http::PathPattern;

use self::tree::{MatchResult, Pattern, RadixNode};

pub mod tree;

#[derive(Clone, Debug, Default)]
pub struct Router<T> {
    tree: HashMap<Method, RadixNode<T>>,
}

impl<T> Router<T> {
    pub fn new() -> Self {
        Router {
            tree: Default::default(),
        }
    }
    pub fn add_route(&mut self, method: Method, path: Vec<PathPattern>, data: T) -> bool {
        let node = self.tree.entry(method).or_default();
        let path: Vec<Pattern> = path
            .into_iter()
            .map(|pattern| match pattern {
                PathPattern::Literal(literal) => Pattern::Static(literal.0),
                PathPattern::Var(_) => Pattern::Variable,
            })
            .collect();

        node.insert_path(&path, data).is_ok()
    }

    pub fn check_path<'a, 'b>(
        &'a self,
        method: &Method,
        path: &'b str,
    ) -> Option<MatchResult<'a, 'b, T>> {
        let node = self.tree.get(method)?;
        let result = node.matches_str(path)?;
        Some(result)
    }
}
