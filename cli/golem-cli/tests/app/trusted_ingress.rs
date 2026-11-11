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

use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::formatdoc;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

const TRUSTED_INGRESS_ADDRESSES: &str =
    "GOLEM__WORKER_SERVICE__ROUTE_RESOLVER__TRUSTED_INGRESS_ADDRESSES";

/// Deploys an HTTP router that answers every request with the request URL that its agent sees.
async fn deploy_origin_probe(ctx: &mut TestContext) {
    assert!(
        ctx.cli([flag::YES, cmd::NEW, "origin-probe", flag::TEMPLATE, "ts"])
            .await
            .success_or_dump()
    );
    ctx.cd("origin-probe");
    fs::write_str(
        ctx.cwd_path_join("src/counter-agent.ts"),
        include_str!("trusted_ingress_router.ts"),
    )
    .unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: origin-probe
        environments:
          local:
            server: local
            componentPresets: quick
        components:
          origin-probe:main:
            templates: ts
        httpApi:
          deployments:
            local:
              - domain: localhost:9006
                scheme: https
                agents:
                  OriginProbe: {{}}
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());
}

#[test]
#[timeout("10 minutes")]
async fn trusted_ingress_range_gives_agents_the_forwarded_origin() {
    let mut ctx = TestContext::new();
    ctx.add_env_var(TRUSTED_INGRESS_ADDRESSES, r#"["127.0.0.0/8"]"#);
    ctx.start_server().await;
    deploy_origin_probe(&mut ctx).await;
    let port = ctx.custom_request_port();
    // The Host header is 127.0.0.1:<port>, not the deployment domain, so the request reaches the
    // deployment only through X-Forwarded-Host.
    let url = format!("http://127.0.0.1:{port}/probe/check?x=1");
    let client = reqwest::Client::new();

    let forwarded = client
        .get(&url)
        .header("x-forwarded-proto", "https")
        .header("x-forwarded-host", format!("localhost:{port}"))
        .send()
        .await
        .unwrap();
    assert_eq!(forwarded.status(), 200);
    assert_eq!(
        forwarded.text().await.unwrap(),
        format!("https://localhost:{port}/probe/check?x=1")
    );

    // A trusted peer must send both headers.
    let proto_only = client
        .get(&url)
        .header("x-forwarded-proto", "https")
        .send()
        .await
        .unwrap();
    assert_eq!(proto_only.status(), 400);
}

#[test]
#[timeout("10 minutes")]
async fn peer_outside_the_trusted_range_cannot_set_the_origin() {
    let mut ctx = TestContext::new();
    // TEST-NET-1 (RFC 5737) is never the address of a local client.
    ctx.add_env_var(TRUSTED_INGRESS_ADDRESSES, r#"["192.0.2.0/24"]"#);
    ctx.start_server().await;
    deploy_origin_probe(&mut ctx).await;
    let port = ctx.custom_request_port();

    let response = reqwest::Client::new()
        .get(format!("http://localhost:{port}/probe/check?x=1"))
        .header("x-forwarded-proto", "https")
        .header("x-forwarded-host", "evil.test")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.text().await.unwrap(),
        format!("http://localhost:{port}/probe/check?x=1")
    );
}
