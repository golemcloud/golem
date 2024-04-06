use hyper::http::Method;
use rustc_hash::FxHashMap;

use crate::api_definition::http::PathPattern;
use tree::{Pattern, RadixNode};

pub mod tree;

#[derive(Clone, Debug, Default)]
pub struct Router<T> {
    tree: FxHashMap<Method, RadixNode<T>>,
}

impl<T> Router<T> {
    pub fn new() -> Self {
        Router {
            tree: Default::default(),
        }
    }

    /// Add a route to the router.
    /// Returns true if the route was added successfully.
    /// False indicates that there is a conflict.
    pub fn add_route(&mut self, method: Method, path: Vec<PathPattern>, data: T) -> bool {
        let node = self.tree.entry(method).or_default();
        let path: Vec<Pattern> = convert_path(path);
        node.insert_path(&path, data).is_ok()
    }

    pub fn get_route(&self, method: &Method, path: &[PathPattern]) -> Option<&T> {
        let node = self.tree.get(method)?;
        let path: Vec<Pattern> = convert_path(path.to_vec());
        let result = node.get(&path)?;
        Some(result)
    }

    pub fn check_path(&self, method: &Method, path: &[&str]) -> Option<&T> {
        let node = self.tree.get(method)?;
        let result = node.matches(path)?;
        Some(result)
    }
}

pub fn parse_path(path: &str) -> Vec<&str> {
    path.trim().trim_matches('/').split('/').collect::<Vec<_>>()
}

fn convert_path(path: Vec<PathPattern>) -> Vec<Pattern> {
    path.into_iter()
        .map(|pattern| match pattern {
            PathPattern::Literal(literal) => Pattern::literal(literal.0),
            PathPattern::Var(_) => Pattern::Variable,
        })
        .collect()
}

#[test]
fn test_router() {
    let mut router = Router::new();

    router.add_route(Method::GET, vec![PathPattern::literal("test")], 1);
    router.add_route(
        Method::GET,
        vec![PathPattern::literal("test"), PathPattern::var("id")],
        2,
    );

    assert_eq!(router.check_path(&Method::GET, &["test"]), Some(&1));
    assert_eq!(router.check_path(&Method::GET, &["test", "123"]), Some(&2));
    assert_eq!(router.check_path(&Method::POST, &["api"]), None);

    router.add_route(Method::POST, vec![PathPattern::literal("api")], 1);

    assert_eq!(router.check_path(&Method::POST, &["api"]), Some(&1));
}
