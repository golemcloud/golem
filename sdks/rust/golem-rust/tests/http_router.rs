// Copyright 2024-2026 Golem Cloud
// Licensed under the Apache License, Version 2.0 (https://www.apache.org/licenses/LICENSE-2.0).

test_r::enable!();

#[test_r::sequential]
mod tests {
    use golem_rust::agentic::{
        AgentStream, AgentTypeName, BaseAgent, Config, Header, HttpRequest, HttpResponse,
        HttpRouter, OriginalHttpRequest, Principal, get_agent_type_by_name,
    };
    use golem_rust::golem_agentic::golem::agent::common::{
        AgentMode, AgentTypeKind, FileMapping, HttpMethod, InputSchema, PathSegment, Snapshotting,
        SystemVariable,
    };
    use golem_rust::schema::SchemaValueStream;
    use golem_rust::schema::stream::SchemaValueStreamSource;
    use golem_rust::{
        ConfigSchema, FromSchema, IntoSchema, SchemaValue, agent_definition, agent_implementation,
        http_router,
    };
    use test_r::test;

    struct Echo;
    #[http_router(name = "SdkEcho", mount = "/echo", auth = true, cors = ["https://example.com"])]
    impl HttpRouter for Echo {
        type Config = ();
        fn new(_: Config<()>) -> Self {
            Self
        }
        async fn handle(&self, request: HttpRequest) -> HttpResponse {
            HttpResponse {
                status: 201,
                headers: request.headers,
                body: request.body,
            }
        }
    }

    struct Static;
    #[http_router(name = "SdkStatic", mount = "/", static_files = [
        ("/assets/*", "/primary/$1"), ("/assets/*", "/fallback/$1"),
        ("/assets/a", "/exact"), ("/", "/root.txt"),
    ])]
    impl HttpRouter for Static {
        type Config = ();
        fn new(_: Config<()>) -> Self {
            Self
        }
    }

    #[allow(dead_code)]
    #[derive(ConfigSchema)]
    struct Settings {
        title: String,
    }
    struct Provider;
    #[http_router(name = "SdkProvider", mount = "/docs")]
    impl HttpRouter for Provider {
        type Config = Settings;
        fn new(_: Config<Settings>) -> Self {
            Self
        }
        async fn openapi(&self) -> String {
            r#"{"openapi":"3.1.0","info":{"title":"test","version":"1"},"paths":{}}"#.into()
        }
    }

    #[agent_definition(mount = "/files/{agent-type}/{agent-version}/{owner}", filesystem_bindings = [("/*", "/public/$1")])]
    trait Files {
        fn new(owner: String) -> Self;
        fn ping(&self) -> String;
    }
    struct FilesImpl;
    #[agent_implementation]
    impl Files for FilesImpl {
        fn new(_owner: String) -> Self {
            Self
        }
        fn ping(&self) -> String {
            "pong".into()
        }
    }

    #[test]
    fn generated_roles_and_provisioning_identity() {
        let get = |name: &str| get_agent_type_by_name(&AgentTypeName(name.into())).unwrap();
        for name in ["SdkEcho", "SdkStatic", "SdkProvider"] {
            let metadata = get(name);
            assert!(matches!(metadata.kind, AgentTypeKind::HttpRouter));
            assert!(matches!(metadata.mode, AgentMode::Ephemeral));
            assert!(matches!(metadata.snapshotting, Snapshotting::Disabled));
            let InputSchema::Parameters(params) = metadata.constructor.input_schema;
            assert!(params.is_empty());
        }
        let echo = get("SdkEcho");
        assert_eq!(echo.methods.len(), 1);
        assert_eq!(echo.methods[0].name, "handle");
        assert!(matches!(
            echo.methods[0].http_endpoint[0].http_method,
            HttpMethod::Any
        ));
        assert!(echo.methods[0].http_endpoint[0].auth_details.is_none());
        assert!(echo.http_mount.unwrap().auth_details.unwrap().required);
        let static_only = get("SdkStatic");
        assert!(static_only.methods.is_empty(), "metadata-static-only");
        let mount = static_only.http_mount.unwrap();
        assert!(mount.path_prefix.is_empty());
        assert_eq!(mount.static_bindings.len(), 4);
        let FileMapping::Subtree(first) = &mount.static_bindings[0] else {
            panic!()
        };
        let FileMapping::Subtree(second) = &mount.static_bindings[1] else {
            panic!()
        };
        assert_eq!(first.filesystem_root, "/primary");
        assert_eq!(second.filesystem_root, "/fallback");
        let provider = get("SdkProvider");
        assert_eq!(provider.methods.len(), 1, "metadata-provider-only");
        assert_eq!(provider.methods[0].name, "openapi");
        assert!(provider.methods[0].http_endpoint.is_empty());
        assert_eq!(
            provider
                .http_mount
                .unwrap()
                .openapi_provider_method
                .as_deref(),
            Some("openapi")
        );
        assert_eq!(provider.config.len(), 1, "tooling-router-config");
        assert_eq!(provider.config[0].path, ["title"]);
        assert!(matches!(get("Files").kind, AgentTypeKind::Regular));
        let mount = get("Files").http_mount.unwrap();
        assert!(matches!(
            mount.path_prefix[1],
            PathSegment::SystemVariable(SystemVariable::AgentType)
        ));
        assert!(matches!(
            mount.path_prefix[2],
            PathSegment::SystemVariable(SystemVariable::AgentVersion)
        ));
        assert!(
            matches!(&mount.path_prefix[3], PathSegment::PathVariable(variable) if variable.variable_name == "owner")
        );
        assert_eq!(
            get("Files").methods[0].name,
            "ping",
            "tooling-regular-files-still-callable"
        );
        let _ = std::mem::size_of::<FilesClient>();
    }

    struct Unpolled;
    impl SchemaValueStreamSource for Unpolled {
        fn next(
            &mut self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Option<golem_rust::schema::wit::wire::SchemaValueTree>,
                            String,
                        >,
                    > + Send
                    + '_,
            >,
        > {
            panic!("HTTP adapter or dispatch polled the body")
        }
    }
    fn request() -> HttpRequest {
        HttpRequest {
            method: "mIxEd".into(),
            scheme: "https".into(),
            authority: "example.com:8443".into(),
            path: "/echo/%61/".into(),
            query: Some("a=1&a=2+3&x=%2f".into()),
            headers: vec![Header {
                name: "x-byte".into(),
                value: vec![0x80, 0xff],
            }],
            body: AgentStream::from_schema_stream(
                SchemaValueStream::from_source(Unpolled),
                |value| Vec::<u8>::from_value(&value).map_err(|e| e.to_string()),
            ),
        }
    }

    #[test]
    async fn ordinary_dispatch_forwards_body_without_polling() {
        let input = request();
        let body = input.body.to_value();
        let mut echo = <Echo as HttpRouter>::new(Config::new());
        let output = echo
            .invoke(
                "handle".into(),
                SchemaValue::Record {
                    fields: vec![input.to_value()],
                },
                Principal::Anonymous,
            )
            .await
            .unwrap();
        let output = HttpResponse::from_value(&output.value.unwrap()).unwrap();
        assert_eq!(output.status, 201);
        assert_eq!(output.body.to_value(), body);
        assert_eq!(output.headers[0].value, [0x80, 0xff]);
        let mut provider = <Provider as HttpRouter>::new(Config::new());
        let document = provider
            .invoke(
                "openapi".into(),
                SchemaValue::Record { fields: vec![] },
                Principal::Anonymous,
            )
            .await
            .unwrap();
        assert!(
            String::from_value(&document.value.unwrap())
                .unwrap()
                .contains("3.1.0")
        );
        assert!(
            provider
                .invoke(
                    "handle".into(),
                    SchemaValue::Record { fields: vec![] },
                    Principal::Anonymous
                )
                .await
                .is_err()
        );
    }

    #[test]
    fn rust_http_views_preserve_bytes_queries_and_stream_identity() {
        for query in [None, Some(String::new()), Some("a=1&a=2+3&x=%2f".into())] {
            let mut original = request();
            original.query = query.clone();
            let body = original.body.to_value();
            let adapted = original.into_http().unwrap();
            assert_eq!(adapted.method().as_str(), "mIxEd");
            assert_eq!(adapted.uri().path(), "/echo/%61/");
            assert_eq!(
                adapted
                    .extensions()
                    .get::<OriginalHttpRequest>()
                    .unwrap()
                    .query,
                query
            );
            assert_eq!(adapted.headers()["x-byte"].as_bytes(), &[0x80, 0xff]);
            assert_eq!(adapted.headers()["host"], "example.com:8443");
            assert_eq!(adapted.body().to_value(), body);
            let response = http::Response::builder()
                .status(202)
                .header("set-cookie", "first=1")
                .header("set-cookie", vec![b'x', b'=', 0x80])
                .body(adapted.into_body())
                .unwrap();
            let response = HttpResponse::from(response);
            assert_eq!(response.headers.len(), 2, "envelope-cookie-order");
            assert_eq!(response.headers[0].value, b"first=1");
            assert_eq!(response.headers[1].value, [b'x', b'=', 0x80]);
            assert_eq!(response.body.to_value(), body);
        }
    }

    fn corpus() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
        ))
        .unwrap()
    }

    fn unhex(value: &str) -> Vec<u8> {
        (0..value.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&value[i..i + 2], 16).unwrap())
            .collect()
    }

    struct Chunks {
        items: std::collections::VecDeque<Result<Vec<u8>, String>>,
        polls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    impl Drop for Chunks {
        fn drop(&mut self) {
            self.dropped
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    impl SchemaValueStreamSource for Chunks {
        fn next(
            &mut self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Option<golem_rust::schema::wit::wire::SchemaValueTree>,
                            String,
                        >,
                    > + Send
                    + '_,
            >,
        > {
            self.polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move {
                self.items
                    .pop_front()
                    .transpose()?
                    .map(|bytes| {
                        golem_rust::schema::wit::encode_value(&bytes.to_value())
                            .map_err(|e| e.to_string())
                    })
                    .transpose()
            })
        }
    }

    #[test]
    async fn shared_envelope_vectors_and_lazy_disposal() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        let corpus = corpus();
        let case = corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "envelope-extension-and-bytes")
            .unwrap();
        let input = &case["input"];
        let polls = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        let chunks = Chunks {
            items: input["chunks_hex"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| Ok(unhex(s.as_str().unwrap())))
                .chain([Err("stream failure".into())])
                .collect(),
            polls: polls.clone(),
            dropped: dropped.clone(),
        };
        let (path, query) = input["target"].as_str().unwrap().split_once('?').unwrap();
        let mut request = request();
        request.method = input["method"].as_str().unwrap().into();
        request.scheme = input["scheme"].as_str().unwrap().into();
        request.authority = input["authority"].as_str().unwrap().into();
        request.path = path.into();
        request.query = Some(query.into());
        request.headers = input["headers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| Header {
                name: h["name"].as_str().unwrap().to_ascii_lowercase(),
                value: unhex(h["value_hex"].as_str().unwrap()),
            })
            .collect();
        request.body =
            AgentStream::from_schema_stream(SchemaValueStream::from_source(chunks), |v| {
                Vec::<u8>::from_value(&v).map_err(|e| e.to_string())
            });
        let mut adapted = request.into_http().unwrap();
        assert_eq!(polls.load(Ordering::SeqCst), 0, "{}", case["id"]);
        assert_eq!(adapted.method().as_str(), case["expect"]["method"]);
        assert_eq!(adapted.uri().path(), case["expect"]["path"]);
        assert_eq!(adapted.uri().query().unwrap(), case["expect"]["query"]);
        let actual = adapted
            .headers()
            .get_all("x-binary")
            .iter()
            .map(|v| v.as_bytes().to_vec())
            .collect::<Vec<_>>();
        let expected = case["expect"]["headers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| unhex(h["value_hex"].as_str().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "{}", case["id"]);
        let mut bytes = Vec::new();
        for (index, chunk) in input["chunks_hex"].as_array().unwrap().iter().enumerate() {
            let next = adapted.body_mut().next().await.unwrap().unwrap();
            assert_eq!(
                next,
                unhex(chunk.as_str().unwrap()),
                "empty chunks are not EOF"
            );
            bytes.extend(next);
            assert_eq!(polls.load(Ordering::SeqCst), index + 1);
        }
        assert_eq!(bytes, unhex(case["expect"]["body_hex"].as_str().unwrap()));
        assert_eq!(
            adapted.body_mut().next().await.unwrap_err(),
            "stream failure"
        );
        drop(adapted);
        assert!(
            dropped.load(Ordering::SeqCst),
            "dropping the adapted body must release its source"
        );

        let case = corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "envelope-cookie-order")
            .unwrap();
        let mut response =
            http::Response::builder().status(case["input"]["status"].as_u64().unwrap() as u16);
        for header in case["input"]["headers"].as_array().unwrap() {
            response = response.header(header[0].as_str().unwrap(), header[1].as_str().unwrap());
        }
        let response = HttpResponse::from(response.body(super::tests::request().body).unwrap());
        let mut names = std::collections::HashSet::new();
        for h in case["expect"]["headers"].as_array().unwrap() {
            names.insert(h[0].as_str().unwrap());
        }
        for name in names {
            let expected = case["expect"]["headers"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|h| h[0] == name)
                .map(|h| h[1].as_str().unwrap().as_bytes())
                .collect::<Vec<_>>();
            let actual = response
                .headers
                .iter()
                .filter(|h| h.name == name)
                .map(|h| h.value.as_slice())
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{}: {name}", case["id"]);
        }
    }
}
