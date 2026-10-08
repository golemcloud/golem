// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
    use serde_json::Value;
    use test_r::test;

    #[test]
    fn shared_complete_typed_routes() {
        assert_shared_typed_routes(&[
            "route-typed-parameter-before-literal-mount",
            "route-typed-404-terminal",
        ]);
    }

    #[test]
    fn typed_literal_dead_end_backtracks_to_parameter_route() {
        assert_shared_typed_routes(&["route-literal-dead-end"]);
    }

    fn assert_shared_typed_routes(ids: &[&str]) {
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/http-handlers/corpus.json"
        ))
        .unwrap();
        for &id in ids {
            let case = corpus["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["id"] == id)
                .unwrap();
            let mut router = Router::new();
            for route in case["input"]["typed"].as_array().unwrap() {
                let path = route["path"].as_str().unwrap();
                let segments = path
                    .strip_prefix('/')
                    .unwrap()
                    .split('/')
                    .map(|segment| {
                        if let Some(name) = segment
                            .strip_prefix('{')
                            .and_then(|segment| segment.strip_suffix('}'))
                        {
                            PathSegment::Variable {
                                display_name: name.to_string(),
                            }
                        } else {
                            PathSegment::Literal {
                                value: segment.to_string(),
                            }
                        }
                    })
                    .collect();
                assert!(
                    router.add_route(
                        Method::from_bytes(route["method"].as_str().unwrap().as_bytes()).unwrap(),
                        segments,
                        route["id"].as_str().unwrap(),
                    ),
                    "{id}"
                );
            }
            let segments = case["input"]["target"]
                .as_str()
                .unwrap()
                .strip_prefix('/')
                .unwrap()
                .split('/')
                .collect::<Vec<_>>();
            let method =
                Method::from_bytes(case["input"]["method"].as_str().unwrap().as_bytes()).unwrap();
            let selected = router.route(&method, &segments);
            assert_eq!(
                selected.as_ref().map(|(route, _)| **route),
                case["expect"]["selected"].as_str(),
                "{id}"
            );
            let expected_route = case["input"]["typed"]
                .as_array()
                .unwrap()
                .iter()
                .find(|route| route["id"] == case["expect"]["selected"])
                .unwrap();
            let expected_captures = expected_route["path"]
                .as_str()
                .unwrap()
                .strip_prefix('/')
                .unwrap()
                .split('/')
                .zip(&segments)
                .filter_map(|(pattern, value)| pattern.starts_with('{').then_some(*value))
                .collect::<Vec<_>>();
            assert_eq!(selected.unwrap().1, expected_captures, "{id}");
        }
    }

    #[test]
    fn typed_root_and_concrete_methods_are_distinct() {
        let mut router = Router::new();
        assert!(router.add_route(Method::GET, vec![], "get-root"));
        assert!(router.add_route(Method::from_bytes(b"ANY").unwrap(), vec![], "extension"));
        assert_eq!(router.route(&Method::GET, &[]), Some((&"get-root", vec![])));
        assert_eq!(router.route(&Method::HEAD, &[]), None);
        assert_eq!(router.route(&Method::POST, &[]), None);
        assert_eq!(
            router.route(&Method::from_bytes(b"ANY").unwrap(), &[]),
            Some((&"extension", vec![]))
        );
    }

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
                PathSegment::Variable {
                    display_name: "unused".into(),
                },
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
