// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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

#[cfg(test)]
mod tests {
    use crate::gateway_execution::router::{Router, RouterPattern};
    use http::Method;
    use test_r::test;

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
}
