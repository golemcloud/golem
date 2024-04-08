use hyper::Method;

use super::{tree::RadixNode, RouterPattern};

#[derive(Clone, Debug, Default)]
pub struct Router<T> {
    tree: rustc_hash::FxHashMap<Method, RadixNode<T>>,
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
    pub fn add_route(&mut self, method: Method, path: Vec<RouterPattern>, data: T) -> bool {
        let node = self.tree.entry(method).or_default();
        node.insert_path(&path, data).is_ok()
    }

    pub fn get_route(&self, method: &Method, path: &[RouterPattern]) -> Option<&T> {
        let node = self.tree.get(method)?;
        let result = node.get(path)?;
        Some(result)
    }

    pub fn check_path(&self, method: &Method, path: &[&str]) -> Option<&T> {
        let node = self.tree.get(method)?;
        let result = node.matches(path)?;
        Some(result)
    }
}

#[test]
fn test_router() {
    let mut router = Router::new();

    router.add_route(Method::GET, vec![RouterPattern::literal("test")], 1);
    router.add_route(
        Method::GET,
        vec![RouterPattern::literal("test"), RouterPattern::Variable],
        2,
    );

    assert_eq!(router.check_path(&Method::GET, &["test"]), Some(&1));
    assert_eq!(router.check_path(&Method::GET, &["test", "123"]), Some(&2));
    assert_eq!(router.check_path(&Method::POST, &["api"]), None);

    router.add_route(Method::POST, vec![RouterPattern::literal("api")], 1);

    assert_eq!(router.check_path(&Method::POST, &["api"]), Some(&1));
}
