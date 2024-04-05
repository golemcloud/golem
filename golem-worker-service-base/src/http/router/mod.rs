use hyper::http::Method;
use std::collections::HashMap;

use crate::api_definition::http::PathPattern;
use tree::{Pattern, RadixNode};

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
                PathPattern::Literal(literal) => Pattern::literal(literal.0),
                PathPattern::Var(_) => Pattern::Variable,
            })
            .collect();

        node.insert_path(&path, data).is_ok()
    }

    pub fn check_path(&self, method: &Method, path: &[&str]) -> Option<&T> {
        let node = self.tree.get(method)?;
        let result = node.matches(&path)?;
        Some(result)
    }
}

pub fn parse_path(path: &str) -> Vec<&str> {
    path.trim().trim_matches('/').split('/').collect::<Vec<_>>()
}
