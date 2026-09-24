// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use super::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::formatdoc;
use reqwest::{Body, Method};
use serde_json::Value;
use test_r::{test, timeout};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

fn corpus() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!(
        "../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    ))
    .unwrap()["cases"]
        .as_array()
        .unwrap()
        .clone()
}

fn bytes(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn header(value: &Value) -> (String, Vec<u8>) {
    if value.is_array() {
        (
            value[0].as_str().unwrap().into(),
            value[1].as_str().unwrap().as_bytes().to_vec(),
        )
    } else {
        (
            value["name"].as_str().unwrap().into(),
            bytes(value["value_hex"].as_str().unwrap()),
        )
    }
}

fn scala_bytes(value: &[u8]) -> String {
    format!(
        "Array[Byte]({})",
        value
            .iter()
            .map(|b| format!("{}.toByte", b))
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Translate fixture inputs into a real Scala guest, never its expected observations.
fn corpus_router_source(cases: &[Value]) -> String {
    let branches = cases.iter().filter(|c| c["suite"] == "envelope" && c["input"]["action"] == "response").map(|c| {
        let input = &c["input"];
        let headers = input["headers"].as_array().unwrap().iter().map(|h| {
            let (name, value) = header(h);
            format!("HttpHeader({name:?}, {})", scala_bytes(&value))
        }).collect::<Vec<_>>().join(",");
        let stream = if input["body_producer"] == "fails-if-polled" {
            "AgentStream.fromPull[Array[Byte]](() => Future.failed(new IllegalStateException(\"body polled\")))".into()
        } else {
            let chunks = input["chunks_hex"].as_array().into_iter().flatten().map(|c| scala_bytes(&bytes(c.as_str().unwrap()))).collect::<Vec<_>>().join(",");
            format!("stream(List({chunks}))")
        };
        format!("case \"/corpus/{}\" => HttpResponse(UShort({}), List({headers}), {stream})", c["id"].as_str().unwrap(), input["status"])
    }).collect::<Vec<_>>().join("\n");
    formatdoc! {r#"
        package example.integrationtests
        import golem.{{BaseAgent, UShort}}
        import golem.runtime.annotations.*
        import golem.runtime.http.*
        import golem.schema.AgentStream
        import scala.concurrent.Future
        @httpRouter("ScalaCorpusRouter", "/")
        trait ScalaCorpusRouter extends BaseAgent {{
          @httpHandler def serve(request: HttpRequest): HttpResponse
        }}
        @agentImplementation()
        final class ScalaCorpusRouterImpl() extends ScalaCorpusRouter {{
          private def stream(chunks: List[Array[Byte]]): AgentStream[Array[Byte]] = {{
            var remaining = chunks
            AgentStream.fromPull(() => {{
              val next = remaining.headOption
              remaining = remaining.drop(1)
              Future.successful(next)
            }})
          }}
          def serve(request: HttpRequest): HttpResponse = request.path match {{
            {branches}
            case _ => HttpResponse(UShort(200), List(
              HttpHeader.ascii("x-method", request.method),
              HttpHeader.ascii("x-path", request.path),
              HttpHeader.ascii("x-query", request.query.getOrElse("<absent>"))
            ) ++ request.headers.filter(_.name == "x-binary"), request.body)
          }}
        }}
    "#}
}

#[test]
#[timeout("20 minutes")]
async fn test_scala_http_router_e2e() -> anyhow::Result<()> {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    fs::create_dir_all(ctx.cwd_path_join("scala-http-router"))?;
    ctx.cd("scala-http-router");
    let output = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            ".",
            flag::TEMPLATE,
            "scala",
            flag::COMPONENT_NAME,
            "scala-http-router:main",
        ])
        .await;
    assert!(output.success_or_dump());
    let sources = ctx.cwd_path_join("src/main/scala");
    std::fs::remove_dir_all(&sources)?;
    fs::create_dir_all(sources.join("example/integrationtests"))?;
    fs::write_str(
        sources.join("example/integrationtests/HttpRouterExample.scala"),
        include_str!(
            "../../../../sdks/scala/test-agents/src/main/scala/example/integrationtests/HttpRouterExample.scala"
        ),
    )?;
    let cases = corpus();
    fs::write_str(
        sources.join("example/integrationtests/ScalaCorpusRouter.scala"),
        corpus_router_source(&cases),
    )?;
    fs::write_str(ctx.cwd_path_join("index.html"), "immutable scala file\n")?;
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: scala-http-router
        environments:
          local:
            server: local
        components:
          scala-http-router:main:
            templates: scala
        agents:
          ScalaWebsite:
            config:
              greeting: configured-scala
            files:
              - sourcePath: ./index.html
                targetPath: /site/index.html
                permissions: read-only
        httpApi:
          deployments:
            local:
              - domain: localhost:{port}
                agents:
                  ScalaWebsite: {{}}
                  HttpDocuments: {{}}
                  ScalaCorpusRouter: {{}}
    "#, version = versions::sdk::MANIFEST, port = ctx.custom_request_port()},
    )?;
    let output = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(output.success_or_dump());
    let client = reqwest::Client::new();
    let base = format!("http://localhost:{}", ctx.custom_request_port());

    for path in ["/scala", "/scala/assets/index.html"] {
        let response = client.get(format!("{base}{path}")).send().await?;
        assert_eq!(response.status(), 200, "{path}");
        assert!(
            response.headers()["etag"]
                .to_str()?
                .starts_with("\"blake3-")
        );
        assert_eq!(response.text().await?, "immutable scala file\n");
    }
    let response = client
        .get(format!("{base}/scala-documents/alice/latest"))
        .send()
        .await?;
    if !response.status().is_success() {
        let diagnostic = ctx
            .cli([cmd::AGENT, "oplog", "HttpDocuments(\"alice\")"])
            .await;
        let _ = diagnostic.success_or_dump();
    }
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.text().await?, "created:alice");
    assert_eq!(
        client
            .get(format!("{base}/scala/dependency"))
            .send()
            .await?
            .text()
            .await?,
        "updated"
    );
    assert_eq!(
        client
            .get(format!("{base}/scala-documents/from-router/latest"))
            .send()
            .await?
            .text()
            .await?,
        "configured-scala"
    );
    let openapi: Value = client
        .get(format!("{base}/openapi.json"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        openapi["paths"]["/scala/echo"]["post"]["operationId"],
        "scalaEcho"
    );

    let (tx, rx) = mpsc::channel::<Result<Vec<u8>, std::io::Error>>(1);
    tx.send(Ok(vec![0, 128, 255, 42])).await?;
    let mut response = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        client
            .request(
                Method::from_bytes(b"x-Custom")?,
                format!("{base}/scala/echo?q=+&q=%20"),
            )
            .body(Body::wrap_stream(ReceiverStream::new(rx)))
            .send(),
    )
    .await??;
    assert_eq!(response.headers()["x-method"], "x-Custom");
    assert_eq!(response.headers()["x-query"], "q=+&q=%20");
    let cookies = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.as_bytes().to_vec())
        .collect::<Vec<_>>();
    assert_eq!(cookies, vec![b"first=one".to_vec(), vec![115, 61, 128]]);
    let mut first = Vec::new();
    while first.len() < 4 {
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(30), response.chunk())
            .await??
            .unwrap();
        first.extend_from_slice(&chunk);
    }
    assert_eq!(first, &[0, 128, 255, 42]);
    tx.send(Ok(vec![19, 23])).await?;
    drop(tx);
    assert_eq!(response.bytes().await?.as_ref(), &[19, 23]);

    let case = cases
        .iter()
        .find(|c| c["id"] == "envelope-extension-and-bytes")
        .unwrap();
    let input = &case["input"];
    let expect = &case["expect"];
    let mut request = client.request(
        Method::from_bytes(input["method"].as_str().unwrap().as_bytes())?,
        format!("{base}{}", input["target"].as_str().unwrap()),
    );
    for h in input["headers"].as_array().unwrap() {
        let (name, value) = header(h);
        request = request.header(
            reqwest::header::HeaderName::from_bytes(name.as_bytes())?,
            reqwest::header::HeaderValue::from_bytes(&value)?,
        );
    }
    let chunks = input["chunks_hex"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| Ok::<_, std::io::Error>(bytes(c.as_str().unwrap())))
        .collect::<Vec<_>>();
    let response = request
        .body(Body::wrap_stream(tokio_stream::iter(chunks)))
        .send()
        .await?;
    for (name, field) in [
        ("x-method", "method"),
        ("x-path", "path"),
        ("x-query", "query"),
    ] {
        assert_eq!(
            response.headers()[name],
            expect[field].as_str().unwrap(),
            "{}",
            case["id"]
        );
    }
    let expected_headers = expect["headers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| header(h).1)
        .collect::<Vec<_>>();
    assert_eq!(
        response
            .headers()
            .get_all("x-binary")
            .iter()
            .map(|h| h.as_bytes().to_vec())
            .collect::<Vec<_>>(),
        expected_headers
    );
    assert_eq!(
        response.bytes().await?.as_ref(),
        bytes(expect["body_hex"].as_str().unwrap())
    );

    for case in cases
        .iter()
        .filter(|c| c["suite"] == "envelope" && c["input"]["action"] == "response")
    {
        let id = case["id"].as_str().unwrap();
        let expect = &case["expect"];
        let response = client
            .request(
                Method::from_bytes(case["input"]["method"].as_str().unwrap().as_bytes())?,
                format!("{base}/corpus/{id}"),
            )
            .send()
            .await?;
        assert_eq!(
            response.status().as_u16() as u64,
            expect["status"].as_u64().unwrap(),
            "{id}"
        );
        if let Some(headers) = expect["headers"].as_array() {
            let mut expected = std::collections::BTreeMap::<String, Vec<Vec<u8>>>::new();
            for h in headers {
                let (name, value) = header(h);
                expected.entry(name).or_default().push(value);
            }
            for (name, values) in expected {
                let actual = response
                    .headers()
                    .get_all(&name)
                    .iter()
                    .map(|v| v.as_bytes().to_vec())
                    .collect::<Vec<_>>();
                assert_eq!(actual, values, "{id}: {name}");
            }
        }
        if let Some(headers) = expect["absent_headers"].as_array() {
            for name in headers {
                assert!(
                    !response.headers().contains_key(name.as_str().unwrap()),
                    "{id}"
                );
            }
        }
        if let Some(body) = expect["body_hex"].as_str() {
            assert_eq!(response.bytes().await?.as_ref(), bytes(body), "{id}");
        }
    }
    assert_eq!(
        client
            .get(format!("{base}/scala/fail-before"))
            .send()
            .await?
            .status(),
        502
    );
    let (tx, rx) = mpsc::channel::<Result<Vec<u8>, std::io::Error>>(1);
    tx.send(Ok(vec![41])).await?;
    let mut response = client
        .post(format!("{base}/scala/fail-after"))
        .body(Body::wrap_stream(ReceiverStream::new(rx)))
        .send()
        .await?;
    assert_eq!(response.status(), 200);
    assert_eq!(response.chunk().await?.unwrap().as_ref(), &[41]);
    tx.send(Ok(vec![42])).await?;
    drop(tx);
    assert!(
        response.bytes().await.is_err(),
        "producer failure after public commitment must not become successful EOF"
    );

    let (tx, rx) = mpsc::channel::<Result<Vec<u8>, std::io::Error>>(1);
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        client
            .post(format!("{base}/scala/early"))
            .body(Body::wrap_stream(ReceiverStream::new(rx)))
            .send(),
    )
    .await??;
    assert_eq!(response.text().await?, "early");
    drop(tx);
    Ok(())
}
