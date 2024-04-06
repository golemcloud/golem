use std::borrow::Borrow;

use hyper::http::Method;

use crate::api_definition::http::PathPattern;
use tree::{Pattern, RadixNode};

use self::{method::MethodOrd, vecmap::VecMap};

pub mod tree;
pub mod vecmap;

#[derive(Clone, Debug, Default)]
pub struct Router<T> {
    tree: VecMap<MethodOrd, RadixNode<T>>,
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
        let method_ord: &MethodOrd = method.borrow();
        let node = self.tree.get(method_ord)?;
        let path: Vec<Pattern> = convert_path(path.to_vec());
        let result = node.get(&path)?;
        Some(result)
    }

    pub fn check_path(&self, method: &Method, path: &[&str]) -> Option<&T> {
        let method_ord: &MethodOrd = method.borrow();
        let node = self.tree.get(method_ord)?;
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
            match (&self.0, &other.0) {
                (&Method::OPTIONS, &Method::OPTIONS) => std::cmp::Ordering::Equal,
                (&Method::OPTIONS, _) => std::cmp::Ordering::Less,
                (_, &Method::OPTIONS) => std::cmp::Ordering::Greater,

                (&Method::GET, &Method::GET) => std::cmp::Ordering::Equal,
                (&Method::GET, _) => std::cmp::Ordering::Less,
                (_, &Method::GET) => std::cmp::Ordering::Greater,

                (&Method::POST, &Method::POST) => std::cmp::Ordering::Equal,
                (&Method::POST, _) => std::cmp::Ordering::Less,
                (_, &Method::POST) => std::cmp::Ordering::Greater,

                (&Method::PUT, &Method::PUT) => std::cmp::Ordering::Equal,
                (&Method::PUT, _) => std::cmp::Ordering::Less,
                (_, &Method::PUT) => std::cmp::Ordering::Greater,

                (&Method::DELETE, &Method::DELETE) => std::cmp::Ordering::Equal,
                (&Method::DELETE, _) => std::cmp::Ordering::Less,
                (_, &Method::DELETE) => std::cmp::Ordering::Greater,

                (&Method::HEAD, &Method::HEAD) => std::cmp::Ordering::Equal,
                (&Method::HEAD, _) => std::cmp::Ordering::Less,
                (_, &Method::HEAD) => std::cmp::Ordering::Greater,

                (&Method::TRACE, &Method::TRACE) => std::cmp::Ordering::Equal,
                (&Method::TRACE, _) => std::cmp::Ordering::Less,
                (_, &Method::TRACE) => std::cmp::Ordering::Greater,

                (&Method::CONNECT, &Method::CONNECT) => std::cmp::Ordering::Equal,
                (&Method::CONNECT, _) => std::cmp::Ordering::Less,
                (_, &Method::CONNECT) => std::cmp::Ordering::Greater,

                (&Method::PATCH, &Method::PATCH) => std::cmp::Ordering::Equal,
                (&Method::PATCH, _) => std::cmp::Ordering::Less,
                (_, &Method::PATCH) => std::cmp::Ordering::Greater,

                // Compare extensions lexicographically
                (left, right) => left.as_str().cmp(right.as_str()),
            }
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
