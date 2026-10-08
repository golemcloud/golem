use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

#[test]
#[timeout("600s")]
async fn moonbit_http_router(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    fs::create_dir_all(ctx.cwd_path_join("moonbit-http")).unwrap();
    ctx.cd("moonbit-http");
    let output = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            ".",
            flag::TEMPLATE,
            "moonbit",
            flag::COMPONENT_NAME,
            "moonbit-http:site",
        ])
        .await;
    assert!(output.success_or_dump());
    fs::write_str(
        ctx.cwd_path_join("counter.mbt"),
        concat!(
            include_str!(
                "../../../../sdks/moonbit/golem_sdk_example1/golem_moonbit_examples/http_router.mbt"
            ),
            "\nfn main {}\n",
        ),
    )
    .unwrap();
    let package = ctx.cwd_path_join("moon.pkg");
    let source = fs::read_to_string(&package).unwrap().replace(
        "import {",
        "import {\n  \"golemcloud/golem_sdk/http\",\n  \"golemcloud/golem_sdk/config\",\n  \"golemcloud/golem_sdk/filesystem\" @fs,\n  \"moonbitlang/core/encoding/utf8\",",
    );
    fs::write_str(&package, source).unwrap();
    fs::write_str(ctx.cwd_path_join("asset.txt"), "immutable asset").unwrap();
    fs::write_str(ctx.cwd_path_join("probe.mbt"), indoc! {r#"
        #derive.golem_schema
        struct MyRequest {
          method : String
          scheme : String
          authority : String
          path : String
          query : String?
          headers : Array[@http.HttpHeader]
          body : @schema.AgentStream[Array[Byte]]
        }
        #derive.http_router
        #derive.mount("/probe")
        struct Probe {}
        fn Probe::new() -> Probe { Probe::{} }
        suberror ProbeError { Failed }
        #derive.http_handler
        pub fn Probe::serve(_self : Self, request : MyRequest) -> @http.HttpResponse raise ProbeError {
          if request.path == "/probe/before" { request.body.drop(); raise Failed }
          let input = request.body
          {
            status: 200,
            headers: [],
            body: @schema.AgentStream::produce(async fn(writer) {
              defer input.drop()
              if writer.write_one([0x61, 0x62]) is @schema.PeerDropped { return }
              while input.read() is Some(_) {}
              raise Failed
            }, on_unstarted_drop=() => input.drop()),
          }
        }
        #derive.config
        struct DocumentConfig { title : String }
        #derive.http_router
        #derive.mount("/document")
        struct DocumentOnly { title : String }
        fn DocumentOnly::new(config : @config.Config[DocumentConfig]) -> DocumentOnly {
          { title: config.value.title }
        }
        #derive.openapi_provider
        pub fn DocumentOnly::spec(self : Self) -> String raise @http.OpenApiJsonError {
          @http.openapi_json({
            "openapi": "3.1.0",
            "info": { "title": self.title.to_json(), "version": "1" },
            "paths": { "/configured": { "get": { "responses": { "200": { "description": self.title.to_json() } } } } },
          })
        }
    "#}).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: moonbit-http
        environments:
          local:
            server: local
            componentPresets: debug
        components:
          moonbit-http:site:
            templates: moonbit
            dir: .
            files:
              - sourcePath: ./asset.txt
                targetPath: /public/message.txt
                permissions: read-only
        httpApi:
          deployments:
            local:
              - domain: localhost:{port}
                agents:
                  Site: {{}}
                  StaticSite: {{}}
                  FileOwner: {{}}
                  Probe: {{}}
                  DocumentOnly: {{}}
        agents:
          DocumentOnly:
            config:
              title: configured-provider
    "#, version = versions::sdk::MANIFEST, port = ctx.custom_request_port()},
    )
    .unwrap();
    let output = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(output.success_or_dump());
    let base = format!("http://localhost:{}", ctx.custom_request_port());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();

    // Ordered mappings fall through on absence; a files-only router needs no handler.
    for path in ["/site/assets/message.txt", "/static/message.txt"] {
        let response = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), "immutable asset");
    }
    let response = client
        .get(format!("{base}/static/missing"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);

    // The streaming envelope preserves arbitrary bytes and repeated cookies.
    let bytes = vec![0, 0x80, 0xff, b'x'];
    let response = client
        .post(format!("{base}/site/echo?"))
        .body(bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let cookies: Vec<_> = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    assert_eq!(cookies, ["first=1", "second=2"]);
    assert_eq!(response.bytes().await.unwrap().as_ref(), bytes);

    // Keep upload EOF gated until an echoed chunk arrives. An implementation
    // buffering either entire body deadlocks here rather than passing this check.
    let (send, receive) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(1);
    send.send(Ok(vec![0x81, 0, 0xfe])).await.unwrap();
    let mut response = client
        .post(format!("{base}/site/echo"))
        .body(reqwest::Body::wrap_stream(
            tokio_stream::wrappers::ReceiverStream::new(receive),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut prefix = Vec::new();
    while prefix.len() < 3 {
        prefix.extend_from_slice(&response.chunk().await.unwrap().unwrap());
    }
    assert_eq!(prefix, [0x81, 0, 0xfe]);
    send.send(Ok(vec![7, 8])).await.unwrap();
    drop(send);
    assert_eq!(response.bytes().await.unwrap().as_ref(), [7, 8]);

    let response = client
        .head(format!("{base}/site/echo"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.bytes().await.unwrap().is_empty());

    // Separate durable identities own separate constructor-created live files.
    for owner in ["alice", "bob"] {
        let response = client
            .get(format!("{base}/files/{owner}/value"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), owner);
    }
    let response = client
        .put(format!("{base}/files/alice/value"))
        .json(&serde_json::json!({"value": "updated"}))
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    for (owner, expected) in [("alice", "updated"), ("bob", "bob")] {
        let response = client
            .get(format!("{base}/files/{owner}/value"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), expected);
    }
    let response = client
        .get(format!("{base}/openapi.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let document: serde_json::Value = response.json().await.unwrap();
    assert!(document["paths"]["/site/echo"]["post"].is_object());
    assert!(document["paths"]["/files/{name}/value"]["put"].is_object());
    assert_eq!(
        document["paths"]["/document/configured"]["get"]["responses"]["200"]["description"],
        "configured-provider"
    );

    let response = client
        .get(format!("{base}/probe/before"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 502);
    let (send, receive) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(1);
    send.send(Ok(vec![1])).await.unwrap();
    let mut response = client
        .post(format!("{base}/probe/after"))
        .body(reqwest::Body::wrap_stream(
            tokio_stream::wrappers::ReceiverStream::new(receive),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(!response.chunk().await.unwrap().unwrap().is_empty());
    drop(send);
    assert!(
        response.bytes().await.is_err(),
        "producer failure must abort, not complete the body"
    );

    // Exercise actual compiled schema extraction for the negative metadata cases.
    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    ))
    .unwrap();
    for id in [
        "metadata-buffered-handler",
        "metadata-streaming-provider",
        "metadata-nominal-schema-not-enough",
    ] {
        let case = corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == id)
            .unwrap();
        let declaration = if case["input"]["provider"].is_string() {
            "#derive.openapi_provider\npub fn Invalid::spec(_self : Self) -> @schema.AgentStream[String] { @schema.AgentStream::produce(async fn(_) {}) }"
        } else if case["input"]["schema_overrides"].is_object() {
            "#derive.golem_schema\nstruct HttpRequest { method : String; scheme : String; authority : String; path : String; query : String?; headers : Array[@http.HttpHeader]; body : Array[Byte] }\n#derive.http_handler\npub fn Invalid::serve(_self : Self, request : HttpRequest) -> @http.HttpResponse { abort(\"not invoked\") }"
        } else {
            "#derive.http_handler\npub fn Invalid::serve(_self : Self, request : String) -> @http.HttpResponse { abort(\"not invoked\") }"
        };
        fs::write_str(ctx.cwd_path_join("invalid.mbt"), format!(
            "#derive.http_router\n#derive.mount(\"/invalid\")\nstruct Invalid {{}}\nfn Invalid::new() -> Invalid {{ Invalid::{{}} }}\n{declaration}\n"
        )).unwrap();
        let output = ctx.cli([cmd::DEPLOY, flag::YES, flag::FORCE_BUILD]).await;
        assert!(
            !output.success_or_dump(),
            "{id}: invalid extracted schema accepted: {}",
            output.stdout_text()
        );
        let diagnostic = if case["expect"]["error"] == "provider-schema" {
            "OpenAPI provider method"
        } else {
            "HTTP router handler"
        };
        assert!(
            output.stdout_contains(diagnostic) || output.stderr_contains(diagnostic),
            "{id}: expected HTTP schema diagnostic"
        );
    }
}
