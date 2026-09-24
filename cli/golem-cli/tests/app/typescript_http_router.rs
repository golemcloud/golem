use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::formatdoc;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

#[test]
#[timeout("10 minutes")]
async fn test_ts_http_router_deployed() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    assert!(
        ctx.cli([flag::YES, cmd::NEW, "http-routers", flag::TEMPLATE, "ts"])
            .await
            .success_or_dump()
    );
    ctx.cd("http-routers");
    fs::write_str(
        ctx.cwd_path_join("src/counter-agent.ts"),
        include_str!("typescript_http_router.ts"),
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("asset.txt"), "immutable asset").unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: http-routers
        environments:
          local:
            server: local
            componentPresets: quick
        components:
          http-routers:main:
            templates: ts
        agents:
          StaticRouter:
            files:
              - sourcePath: ./asset.txt
                targetPath: /assets/value.txt
                permissions: read-only
        httpApi:
          deployments:
            local:
              - domain: localhost:9006
                scheme: http
                openapiEndpoint: /
                agents:
                  WebRouter: {{}}
                  RawRouter: {{}}
                  StaticRouter: {{}}
                  ProviderRouter: {{}}
                  Files: {{}}
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());
    let base = format!("http://localhost:{}", ctx.custom_request_port());
    let client = reqwest::Client::new();
    let asset = client
        .get(format!("{base}/static/value.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(asset.status(), 200);
    assert_eq!(asset.text().await.unwrap(), "immutable asset");
    let initial = client
        .get(format!("{base}/files/alice/value"))
        .send()
        .await
        .unwrap();
    assert_eq!(initial.status(), 200);
    assert_eq!(initial.text().await.unwrap(), "initial:alice");
    assert!(
        ctx.cli([cmd::AGENT, cmd::INVOKE, "Files(\"alice\")", "update"])
            .await
            .success_or_dump()
    );
    assert_eq!(
        client
            .get(format!("{base}/files/alice/value"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "updated"
    );
    let document: serde_json::Value = client
        .get(format!("{base}/openapi.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        document["paths"]["/web/echo"]["post"].is_object(),
        "{document}"
    );
    let response = client
        .request(
            reqwest::Method::from_bytes(b"CuStOm").unwrap(),
            format!("{base}/raw/x?"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let head: serde_json::Value = response.json().await.unwrap();
    assert_eq!(head["method"], "CuStOm");
    assert_eq!(head["query"], "");
    let head = client
        .head(format!("{base}/web/head"))
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 200);
    assert!(head.bytes().await.unwrap().is_empty());
    let early = client
        .post(format!("{base}/web/early"))
        .body("unread input")
        .send()
        .await
        .unwrap();
    assert_eq!(early.status(), 202);
    assert_eq!(early.text().await.unwrap(), "early");

    // The second input chunk is withheld until the first output chunk arrives.
    let (send, receive) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(1);
    send.send(Ok(b"first".to_vec())).await.unwrap();
    let input = futures_util::stream::unfold(receive, |mut receive| async {
        receive.recv().await.map(|chunk| (chunk, receive))
    });
    let mut echo = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        client
            .post(format!("{base}/web/echo"))
            .body(reqwest::Body::wrap_stream(input))
            .send(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(echo.status(), 200);
    assert_eq!(
        echo.headers()
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect::<Vec<_>>(),
        ["a=first", "a=second"]
    );
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(20), echo.chunk())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "first"
    );
    send.send(Ok(b"second".to_vec())).await.unwrap();
    drop(send);
    assert_eq!(echo.bytes().await.unwrap(), "second");
}
