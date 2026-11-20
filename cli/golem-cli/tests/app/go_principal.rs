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
use crate::app::{TestContext, cmd, flag, go_component_dir};
use golem_cli::{fs, versions};
use indoc::formatdoc;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

/// A Go agent reads the caller of each invocation from its context: the CLI's account for a
/// call from the CLI, the calling agent for a call over RPC.
#[test]
#[timeout("15 minutes")]
async fn test_go_principal() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    assert!(
        ctx.cli([flag::YES, cmd::NEW, "go-principal", flag::TEMPLATE, "go"])
            .await
            .success_or_dump()
    );
    ctx.cd("go-principal");

    let component = go_component_dir(&ctx);
    let module = fs::read_to_string(component.join("go.mod"))
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("module ").map(|m| m.trim().to_string()))
        .expect("the component's go.mod names its module");
    fs::remove(component.join("agents")).unwrap();
    fs::write_str(
        component.join("ledger/ledger.go"),
        include_str!("go_principal.go"),
    )
    .unwrap();
    fs::write_str(
        component.join("main.go"),
        format!("package main\n\nimport _ \"{module}/ledger\"\n\nfunc main() {{}}\n"),
    )
    .unwrap();
    let component_dir = component
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|_| component != ctx.cwd_path())
        .unwrap_or(".")
        .to_string();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: go-principal
        environments:
          local:
            server: local
            componentPresets: debug
        components:
          go-principal:main:
            dir: {component_dir}
            templates: go
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());

    let invoke = async |agent: &str, method: &str, args: &[&str]| {
        let mut argv = vec![flag::YES, cmd::AGENT, cmd::INVOKE, agent, method];
        argv.extend_from_slice(args);
        let outputs = ctx.cli(argv).await;
        assert!(outputs.success_or_dump());
        // The result is printed as a string literal; unescape its quotes.
        outputs.stdout_text().replace('\\', "")
    };

    // The CLI creates and calls the first ledger.
    let direct = invoke("LedgerAgent(\"direct\")", "whoami", &[]).await;
    assert!(direct.contains("method=account agent=account"), "{direct}");

    // A relay creates the second ledger over RPC: both principals are the relay, named by its
    // agent ID.
    let created = invoke("RelayAgent(\"r\")", "ask", &["\"created\""]).await;
    let relay = ctx
        .cli([
            flag::YES,
            flag::FORMAT,
            "json",
            cmd::AGENT,
            cmd::GET,
            "RelayAgent(\"r\")",
        ])
        .await;
    assert!(relay.success_or_dump());
    let relay_id = relay
        .stdout_json::<serde_json::Value>()
        .into_iter()
        .find_map(|document| document["metadata"]["agentId"].as_str().map(str::to_string))
        .expect("agent get prints the agent ID");
    assert!(
        created.contains(&format!("method=agent:{relay_id} agent=agent:{relay_id}")),
        "{created}"
    );

    // Calling the CLI's ledger through the relay makes the relay the caller, in both roles.
    let relayed = invoke("RelayAgent(\"r\")", "ask", &["\"direct\""]).await;
    assert!(
        relayed.contains(&format!("method=agent:{relay_id} agent=golem-user")),
        "{relayed}"
    );

    // The oplog of the CLI's ledger shows the roles of each invocation: the CLI's account for
    // its own call, the relay for the relayed one.
    let oplog = ctx
        .cli([cmd::AGENT, "oplog", "LedgerAgent(\"direct\")"])
        .await;
    assert!(oplog.success_or_dump());
    let text = oplog.stdout_text();
    for expected in [
        "original caller:   account (",
        "immediate caller:  account (",
        "original caller:   agent (",
        "immediate caller:  agent (",
        "tokens:            none",
    ] {
        assert!(text.contains(expected), "{expected} in {text}");
    }
    assert!(!text.contains("claims:"), "{text}");

    let json = ctx
        .cli([
            cmd::AGENT,
            "oplog",
            "LedgerAgent(\"direct\")",
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(json.success_or_dump());
    let callers = json
        .stdout_json::<serde_json::Value>()
        .into_iter()
        .filter_map(|entry| entry["entry"]["invocation"].get("caller").cloned())
        .collect::<Vec<_>>();
    assert!(
        callers
            .iter()
            .any(|caller| caller["original"]["kind"] == "account"),
        "{callers:?}"
    );
    for caller in &callers {
        assert_eq!(caller["tokens"], serde_json::json!([]), "{caller}");
        assert!(caller["original"].get("claims").is_none(), "{caller}");
    }

    // Account and agent callers have no claims, also when the reader asks for them.
    let with_claims = ctx
        .cli([
            cmd::AGENT,
            "oplog",
            "LedgerAgent(\"direct\")",
            "--show-claims",
        ])
        .await;
    assert!(with_claims.success_or_dump());
    assert!(!with_claims.stdout_text().contains("claims:"));
}
