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
use indoc::{formatdoc, indoc};
use std::path::Path;
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

inherit_test_dep!(Tracing);

/// The module path a Go component's go.mod declares.
fn go_module(component: &Path) -> String {
    fs::read_to_string(component.join("go.mod"))
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("module ").map(|m| m.trim().to_string()))
        .expect("the component's go.mod names its module")
}

/// Replaces a Go template component's agents with one package of `source`.
fn replace_go_component(component: &Path, package: &str, source: &str) {
    let module = go_module(component);
    fs::remove(component.join("agents")).unwrap();
    fs::write_str(component.join(format!("{package}/{package}.go")), source).unwrap();
    fs::write_str(
        component.join("main.go"),
        format!("package main\n\nimport _ \"{module}/{package}\"\n\nfunc main() {{}}\n"),
    )
    .unwrap();
}

/// A Go tool is called with typed arguments by an agent of its own component
/// through the tool's own declaration, and by a Go agent of another component
/// through the generated guest tool client — a plain command with globals and a
/// tail, a command with stdout and stderr fed from standard input, and declared
/// errors matched on the caller's side. The second caller's binding installs a
/// typed Go middleware that rewrites one command, refuses a value, and passes
/// the output command and both its outputs through untouched.
#[test]
#[timeout("25 minutes")]
async fn test_go_tools_e2e() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    fs::create_dir_all(ctx.cwd_path_join("go-tools")).unwrap();
    ctx.cd("go-tools");
    for component in ["provider", "consumer"] {
        let outputs = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                "go",
                flag::COMPONENT_NAME,
                &format!("go-tools:{component}"),
            ])
            .await;
        assert!(outputs.success_or_dump());
    }
    // Written whole: both templates' CounterAgents are replaced, so the HTTP
    // API deployment the templates declare for them must go too.
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: go-tools
        environments:
          local:
            server: local
            componentPresets: debug
        components:
          go-tools:provider:
            dir: provider
            templates: go
          go-tools:consumer:
            dir: consumer
            templates: go
            dependencies:
              tools:
                - go-tools:provider/vcs
        tools:
          vcs: {{}}
          middleware:
            vcs-policy:
              component: go-tools:provider
        agents:
          VcsSelfCaller:
            tools:
              vcs: {{}}
          VcsUser:
            tools:
              vcs:
                middleware:
                  - name: vcs-policy
                    parameters:
                      forbid: secret
        bridge:
          go:
            internal:
              tools:
                - vcs
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();

    let provider = ctx.cwd_path_join("provider");
    replace_go_component(&provider, "vcs", include_str!("go_tools_provider.go"));
    let consumer = ctx.cwd_path_join("consumer");
    replace_go_component(&consumer, "user", include_str!("go_tools_consumer.go"));
    let go_mod = consumer.join("go.mod");
    fs::write_str(
        &go_mod,
        fs::read_to_string(&go_mod).unwrap()
            + indoc! {r#"

                require golem.local/bridge/vcs-tool-guest-client v0.0.0

                replace golem.local/bridge/vcs-tool-guest-client => ../golem-temp/bridge-sdk/go/internal/vcs-tool-guest-client
            "#},
    )
    .unwrap();

    assert!(ctx.cli([cmd::BUILD]).await.success_or_dump());
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());

    let run = async |agent: &str| {
        let outputs = ctx
            .cli([
                flag::YES,
                cmd::AGENT,
                cmd::INVOKE,
                &format!("{agent}(\"{}\")", Uuid::new_v4()),
                "run",
            ])
            .await;
        assert!(outputs.success_or_dump());
        outputs.stdout_text()
    };

    // The tool sees the principal the calling agent's invocation runs on
    // behalf of: the CLI's user, not the agent itself.
    let own = run("VcsSelfCaller").await;
    assert!(own.contains("ok:.|self|anonymous|golem-user:1"), "{own}");

    let generated = run("VcsUser").await;
    assert!(
        generated.contains("ok:.|checked:fix|ada|golem-user:protected forbidden"),
        "{generated}"
    );
}
