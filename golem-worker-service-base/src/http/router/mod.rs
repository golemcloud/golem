use hyper::http::Method;

use crate::api_definition::http::PathPattern;
use tree::{Pattern, RadixNode};

use self::vec_map::VecMap;

pub mod tree;
mod vec_map;

#[derive(Clone, Debug, Default)]
pub struct Router<T> {
    tree: VecMap<method::MethodOrd, RadixNode<T>>,
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
        let node = self.tree.get_or_default(method::MethodOrd(method));
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

mod method {
    use std::borrow::Borrow;

    use hyper::http::Method;

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct MethodOrd(pub Method);

    impl Ord for MethodOrd {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.0.as_str().cmp(other.0.as_str())
        }
    }

    /// Prevents the need to clone convert between Method and MethodOrd
    impl Borrow<MethodOrd> for Method {
        fn borrow(&self) -> &MethodOrd {
            // Same memory layout
            unsafe { std::mem::transmute(self) }
        }
    }

    impl PartialOrd for MethodOrd {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
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
