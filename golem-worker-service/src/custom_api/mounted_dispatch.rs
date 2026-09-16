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

use super::call_agent::CallAgentHandler;
use super::error::RequestHandlerError;
use super::route_resolver::ResolvedRouteEntry;
use super::{ResponseBody, RichRequest, RichRouteBehaviour, RouteExecutionResult};
use golem_common::model::AgentId;
use golem_common::model::agent::FileMapping;
use golem_service_base::custom_api::RouterFileIndexEntry;
use http::{Method, StatusCode};
use std::collections::HashMap;

#[allow(dead_code)]
pub(super) enum MountFile<'a> {
    Initial(&'a RouterFileIndexEntry),
    Live {
        agent_id: &'a AgentId,
        path: String,
        directory_request: bool,
    },
}

pub(super) trait MountBackend {
    /// None means absent; every response or error terminates the selected mount.
    async fn file(
        &mut self,
        request: &mut RichRequest,
        selected: &ResolvedRouteEntry,
        file: MountFile<'_>,
    ) -> Result<Option<RouteExecutionResult>, RequestHandlerError>;

    async fn handler(
        &mut self,
        request: &mut RichRequest,
        selected: &ResolvedRouteEntry,
    ) -> Result<RouteExecutionResult, RequestHandlerError>;
}

pub(super) struct PendingMountBackend;

impl MountBackend for PendingMountBackend {
    async fn file(
        &mut self,
        _: &mut RichRequest,
        _: &ResolvedRouteEntry,
        _: MountFile<'_>,
    ) -> Result<Option<RouteExecutionResult>, RequestHandlerError> {
        Ok(Some(empty_response(StatusCode::NOT_IMPLEMENTED)))
    }

    async fn handler(
        &mut self,
        _: &mut RichRequest,
        _: &ResolvedRouteEntry,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        Ok(empty_response(StatusCode::NOT_IMPLEMENTED))
    }
}

/// Called only after the selected route's authentication/session middleware succeeds.
pub(super) async fn dispatch_mount(
    request: &mut RichRequest,
    selected: &ResolvedRouteEntry,
    backend: &mut impl MountBackend,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    let (mappings, router, agent_id) = match &selected.route.behavior {
        RichRouteBehaviour::HttpRouter(router) => (&router.static_bindings, Some(router), None),
        RichRouteBehaviour::AgentFilesystem(filesystem) => {
            let agent_id = CallAgentHandler::build_agent_id(
                selected,
                filesystem.component_id,
                &filesystem.agent_type,
                &filesystem.constructor_input,
                &filesystem.constructor_parameters,
                None,
            )?;
            (&filesystem.filesystem_bindings, None, Some(agent_id))
        }
        _ => {
            return Err(RequestHandlerError::invariant_violated(
                "Expected mounted behavior",
            ));
        }
    };
    let relative = &selected.request_target.segments()[selected.route.path.len()..];
    let directory_request = !relative.is_empty() && selected.request_target.trailing_slash();
    if matches!(*request.underlying.method(), Method::GET | Method::HEAD) {
        for mapping in mappings {
            let Some(path) = mapping_target(mapping, relative, directory_request) else {
                continue;
            };
            let file = if let Some(router) = router {
                if let Some(entry) = router.file_index.iter().find(|entry| entry.path == path) {
                    if directory_request {
                        return Ok(empty_response(StatusCode::FORBIDDEN));
                    }
                    MountFile::Initial(entry)
                } else if router.file_index.iter().any(|entry| {
                    entry
                        .path
                        .strip_prefix(path.trim_end_matches('/'))
                        .is_some_and(|rest| rest.starts_with('/'))
                }) {
                    return Ok(empty_response(StatusCode::FORBIDDEN));
                } else {
                    continue;
                }
            } else {
                MountFile::Live {
                    agent_id: agent_id.as_ref().unwrap(),
                    path,
                    directory_request,
                }
            };
            if let Some(response) = backend.file(request, selected, file).await? {
                return Ok(response);
            }
            if router.is_some() {
                // The immutable index promised a blob. Missing storage is not a mapping miss.
                return Ok(empty_response(StatusCode::INTERNAL_SERVER_ERROR));
            }
        }
    }
    if router.is_some_and(|router| router.handler.is_some()) {
        backend.handler(request, selected).await
    } else {
        Ok(empty_response(StatusCode::NOT_FOUND))
    }
}

fn mapping_target(
    mapping: &FileMapping,
    relative: &[String],
    directory_request: bool,
) -> Option<String> {
    match mapping {
        FileMapping::Exact(exact) if !directory_request && exact.public_path == relative => {
            Some(exact.file_path.clone())
        }
        FileMapping::Subtree(subtree) if relative.starts_with(&subtree.public_prefix) => {
            let suffix = &relative[subtree.public_prefix.len()..];
            let path = if suffix.is_empty() {
                subtree.filesystem_root.clone()
            } else {
                format!(
                    "{}/{}",
                    subtree.filesystem_root.trim_end_matches('/'),
                    suffix.join("/")
                )
            };
            Some(path)
        }
        _ => None,
    }
}

fn empty_response(status: StatusCode) -> RouteExecutionResult {
    RouteExecutionResult {
        status,
        headers: HashMap::new(),
        body: ResponseBody::NoBody,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_common::model::agent::AgentFileContentHash;
    use golem_common::schema::{InputSchema, OutputSchema, SchemaGraph, SchemaType};
    use golem_service_base::custom_api::{
        CompiledInputSchema, CompiledOutputSchema, RouteBehaviour, RouterMethod,
    };
    use test_r::test;

    #[derive(Default)]
    struct Backend {
        states: HashMap<String, String>,
        attempts: Vec<(String, bool)>,
        agents: Vec<String>,
        handler_calls: usize,
    }

    impl MountBackend for Backend {
        async fn file(
            &mut self,
            _: &mut RichRequest,
            _: &ResolvedRouteEntry,
            file: MountFile<'_>,
        ) -> Result<Option<RouteExecutionResult>, RequestHandlerError> {
            let (path, directory) = match file {
                MountFile::Initial(entry) => (entry.path.clone(), false),
                MountFile::Live {
                    agent_id,
                    path,
                    directory_request,
                } => {
                    self.agents.push(agent_id.agent_id.clone());
                    (path, directory_request)
                }
            };
            self.attempts.push((path.clone(), directory));
            match self
                .states
                .get(&path)
                .map(String::as_str)
                .unwrap_or("absent")
            {
                "absent" | "missing-blob" => Ok(None),
                "permission" | "directory" | "symlink" => {
                    Ok(Some(empty_response(StatusCode::FORBIDDEN)))
                }
                "initialization" => Err(anyhow::anyhow!("initialization failed").into()),
                "found" => Ok(Some(empty_response(StatusCode::OK))),
                other => panic!("Unexpected file state {other}"),
            }
        }

        async fn handler(
            &mut self,
            _: &mut RichRequest,
            _: &ResolvedRouteEntry,
        ) -> Result<RouteExecutionResult, RequestHandlerError> {
            self.handler_calls += 1;
            Ok(empty_response(StatusCode::ACCEPTED))
        }
    }

    fn index_entry(path: &str) -> RouterFileIndexEntry {
        RouterFileIndexEntry {
            path: path.into(),
            blob_key: AgentFileContentHash(golem_common::model::diff::Hash::empty()),
            size: 3,
            sha256: [7; 32],
        }
    }

    fn handler(name: &str) -> RouterMethod {
        RouterMethod {
            method_name: name.into(),
            input: CompiledInputSchema {
                graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
                input_schema: InputSchema::Parameters(vec![]),
            },
            output: CompiledOutputSchema {
                graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
                output_schema: OutputSchema::Unit,
            },
        }
    }

    #[test]
    async fn shared_mapping_dispatch_corpus() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
        ))
        .unwrap();
        for id in [
            "route-root-empty-suffix",
            "route-mount-root-trailing-slash",
            "route-child-exhaustion-no-parent",
            "route-earlier-subtree-beats-exact",
            "route-absence-advances",
            "route-permission-does-not-advance",
            "route-storage-does-not-advance",
            "route-static-exhaustion-handler",
            "route-exact-file-trailing-slash-miss",
            "route-concrete-method-only",
            "route-plain-options-any",
        ] {
            let case = corpus["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["id"] == id)
                .unwrap();
            let input = &case["input"];
            let mut backend = Backend::default();
            if let Some(files) = input["files"].as_object() {
                for (path, state) in files {
                    backend
                        .states
                        .insert(path.clone(), state.as_str().unwrap().into());
                }
            }
            let mut routes: Vec<_> = input["mounts"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, mount)| {
                    let mut route =
                        test_route(i as i32, mount["path"].as_str().unwrap(), None, "router");
                    let RouteBehaviour::HttpRouter(router) = &mut route.behavior else {
                        unreachable!()
                    };
                    router.static_bindings = mount["mappings"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|m| {
                            FileMapping::compile(m[0].as_str().unwrap(), m[1].as_str().unwrap())
                                .unwrap()
                        })
                        .collect();
                    router.handler = mount["handler"].as_str().map(handler);
                    router.file_index = backend
                        .states
                        .iter()
                        .filter(|(_, state)| *state != "absent")
                        .map(|(path, _)| index_entry(path))
                        .collect();
                    route
                })
                .collect();
            for (i, typed) in input["typed"].as_array().unwrap().iter().enumerate() {
                routes.push(test_route(
                    100 + i as i32,
                    typed["path"].as_str().unwrap(),
                    typed["method"].as_str(),
                    "typed",
                ));
            }
            let resolver = test_resolver(routes);
            let request = poem::Request::builder()
                .uri(input["target"].as_str().unwrap().parse().unwrap())
                .method(input["method"].as_str().unwrap().parse().unwrap())
                .header("host", "example.com")
                .finish();
            let selected = resolver.resolve_matching_route(&request).await.unwrap();
            let result = dispatch_mount(&mut RichRequest::new(request), &selected, &mut backend)
                .await
                .unwrap();
            let expected = &case["expect"];
            let calls = expected["handler_calls"].as_u64().unwrap_or(0);
            let status = expected["status"]
                .as_u64()
                .unwrap_or(if calls == 1 { 202 } else { 200 });
            assert_eq!(u64::from(result.status.as_u16()), status, "{id}");
            assert_eq!(backend.handler_calls as u64, calls, "{id}");
            // Index absence is resolved without a backend call, including HEAD.
            let attempts = expected["attempted_files"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| p.as_str().unwrap())
                .filter(|path| input["files"][*path] != "absent")
                .collect::<Vec<_>>();
            assert_eq!(
                backend
                    .attempts
                    .iter()
                    .map(|(path, _)| path.as_str())
                    .collect::<Vec<_>>(),
                attempts,
                "{id}"
            );
        }
    }

    #[test]
    async fn live_constructor_and_terminal_outcomes_preserve_checked_suffix() {
        use golem_service_base::custom_api::{ConstructorParameter, PathSegmentType};
        for (state, expected) in [
            ("found", 200),
            ("absent", 404),
            ("permission", 403),
            ("directory", 403),
            ("symlink", 403),
            ("initialization", 500),
        ] {
            let mut route = test_route(1, "/users/{id}", None, "filesystem");
            let RouteBehaviour::AgentFilesystem(filesystem) = &mut route.behavior else {
                unreachable!()
            };
            let input =
                InputSchema::Parameters(vec![golem_common::schema::NamedField::user_supplied(
                    "id",
                    SchemaType::u64(),
                )]);
            filesystem.constructor_input = CompiledInputSchema {
                graph: SchemaGraph::anonymous(SchemaType::record(vec![
                    golem_common::schema::NamedFieldType {
                        name: "id".into(),
                        body: SchemaType::u64(),
                        metadata: Default::default(),
                    },
                ])),
                input_schema: input,
            };
            filesystem.constructor_parameters = vec![ConstructorParameter::Path {
                path_segment_index: golem_service_base::model::SafeIndex::new(0),
                parameter_type: PathSegmentType::U64,
            }];
            filesystem.filesystem_bindings =
                FileMapping::compile_list([("/*", "/public/$1"), ("/*", "/other/$1")]).unwrap();
            let resolver = test_resolver(vec![route]);
            for (id, directory) in [("42", true), ("invalid", false)] {
                let request = poem::Request::builder()
                    .uri(
                        format!("/users/{id}/%252e%252e{}", if directory { "/" } else { "" })
                            .parse()
                            .unwrap(),
                    )
                    .header("host", "example.com")
                    .finish();
                let selected = resolver.resolve_matching_route(&request).await.unwrap();
                let mut backend = Backend {
                    states: HashMap::from([("/public/%2e%2e".into(), state.into())]),
                    ..Default::default()
                };
                let result =
                    dispatch_mount(&mut RichRequest::new(request), &selected, &mut backend).await;
                if id == "invalid" {
                    assert!(matches!(
                        result,
                        Err(RequestHandlerError::ValueParsingFailed { .. })
                    ));
                    assert!(backend.attempts.is_empty());
                } else {
                    if expected == 500 {
                        assert!(result.is_err());
                    } else {
                        assert_eq!(result.unwrap().status.as_u16(), expected);
                    }
                    assert_eq!(backend.attempts[0], ("/public/%2e%2e".into(), true));
                    assert_eq!(
                        backend.attempts.len(),
                        if state == "absent" { 2 } else { 1 }
                    );
                    assert_eq!(backend.agents[0], "agent1(42)");
                }
                assert_eq!(backend.handler_calls, 0);
            }
        }
    }

    #[test]
    async fn immutable_directories_and_pending_actions_are_terminal() {
        let mut route = test_route(1, "/m", None, "router");
        let RouteBehaviour::HttpRouter(router) = &mut route.behavior else {
            unreachable!()
        };
        router.static_bindings = FileMapping::compile_list([("/*", "/public/$1")]).unwrap();
        router.file_index = vec![index_entry("/public/a/file")];
        router.handler = Some(handler("fallback"));
        let resolver = test_resolver(vec![route]);
        for (path, status) in [
            ("/m/a", 403),
            ("/m/a/", 403),
            ("/m/a/file/", 403),
            ("/m/a/file", 501),
            ("/m/missing", 501),
        ] {
            let request = poem::Request::builder()
                .uri(path.parse().unwrap())
                .header("host", "example.com")
                .finish();
            let selected = resolver.resolve_matching_route(&request).await.unwrap();
            assert_eq!(
                dispatch_mount(
                    &mut RichRequest::new(request),
                    &selected,
                    &mut PendingMountBackend
                )
                .await
                .unwrap()
                .status
                .as_u16(),
                status,
                "{path}"
            );
        }
    }
}
