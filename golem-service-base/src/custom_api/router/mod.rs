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

pub mod tree;

use self::tree::RadixNode;
use super::PathSegment;
use http::Method;

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
    pub fn add_route(&mut self, method: Method, path: Vec<PathSegment>, data: T) -> bool {
        let node = self.tree.entry(method).or_default();
        node.add_path(&path, data).is_ok()
    }

    pub fn get_route(&self, method: &Method, path: &[PathSegment]) -> Option<&T> {
        let node = self.tree.get(method)?;
        node.get_by_path(path)
    }

    pub fn route(&self, method: &Method, path: &[&str]) -> Option<(&T, Vec<String>)> {
        let node = self.tree.get(method)?;
        node.matches(path)
    }
}

#[cfg(test)]
mod tests {
    use super::Router;
    use crate::custom_api::PathSegment;
    use http::Method;
    use test_r::test;

    #[test]
    fn test_router() {
        let mut router = Router::new();

        router.add_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "test".into(),
            }],
            1,
        );
        router.add_route(
            Method::GET,
            vec![
                PathSegment::Literal {
                    value: "test".into(),
                },
                PathSegment::Variable,
            ],
            2,
        );

        assert_eq!(
            router.route(&Method::GET, &["test"]),
            Some((&1, Vec::new()))
        );
        assert_eq!(
            router.route(&Method::GET, &["test", "123"]),
            Some((&2, vec![String::from("123")]))
        );
        assert_eq!(router.route(&Method::POST, &["api"]), None);

        router.add_route(
            Method::POST,
            vec![PathSegment::Literal {
                value: "api".into(),
            }],
            1,
        );

        assert_eq!(
            router.route(&Method::POST, &["api"]),
            Some((&1, Vec::new()))
        );
    }
}
