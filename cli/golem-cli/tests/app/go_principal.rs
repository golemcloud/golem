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
use std::path::PathBuf;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

/// The Go component directory `golem new` created: the one holding go.mod.
fn go_component_dir(ctx: &TestContext) -> PathBuf {
    std::fs::read_dir(ctx.cwd_path())
        .unwrap()
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .find(|path| path.join("go.mod").exists())
        .unwrap_or_else(|| ctx.cwd_path().to_path_buf())
}

/// A Go agent reads the principal of each invocation from its input struct and
/// its own, the one it was initialized with, from the context: the CLI's for an
/// agent the CLI created, another agent's for one created over RPC.
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
    assert!(
        direct.contains("method=golem-user agent=golem-user"),
        "{direct}"
    );

    // A relay creates the second ledger over RPC: both principals are the relay.
    let created = invoke("RelayAgent(\"r\")", "ask", &["\"created\""]).await;
    assert!(
        created.contains(r#"method=agent:RelayAgent("r") agent=agent:RelayAgent("r")"#),
        "{created}"
    );

    // Calling the CLI's ledger through the relay changes the invocation's
    // principal, not the one the ledger was initialized with.
    let relayed = invoke("RelayAgent(\"r\")", "ask", &["\"direct\""]).await;
    assert!(
        relayed.contains(r#"method=agent:RelayAgent("r") agent=golem-user"#),
        "{relayed}"
    );
}
