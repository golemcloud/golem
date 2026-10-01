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

/// A Go agent inspects and manages agents through the host API: its own
/// metadata, a checkpoint retried until another agent's state is right, a fork
/// joined through a promise, reverting another agent, listing agents with a
/// filter, and reading its own oplog.
#[test]
#[timeout("15 minutes")]
async fn test_go_agent_ops() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    assert!(
        ctx.cli([flag::YES, cmd::NEW, "go-agent-ops", flag::TEMPLATE, "go"])
            .await
            .success_or_dump()
    );
    ctx.cd("go-agent-ops");

    let component = go_component_dir(&ctx);
    let module = fs::read_to_string(component.join("go.mod"))
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("module ").map(|m| m.trim().to_string()))
        .expect("the component's go.mod names its module");
    fs::remove(component.join("agents")).unwrap();
    fs::write_str(
        component.join("ops/ops.go"),
        include_str!("go_agent_ops.go"),
    )
    .unwrap();
    fs::write_str(
        component.join("main.go"),
        format!("package main\n\nimport _ \"{module}/ops\"\n\nfunc main() {{}}\n"),
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
        app: go-agent-ops
        environments:
          local:
            server: local
            componentPresets: debug
        components:
          go-agent-ops:main:
            dir: {component_dir}
            templates: go
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());

    let invoke = async |method: &str, args: &[&str]| {
        let mut argv = vec![
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "OpsAgent(\"o\")",
            method,
        ];
        argv.extend_from_slice(args);
        let outputs = ctx.cli(argv).await;
        assert!(outputs.success_or_dump());
        // The result is printed as a literal; unescape its quotes.
        outputs.stdout_text().replace('\\', "")
    };

    let own = invoke("self", &[]).await;
    assert!(own.contains(r#"OpsAgent("o") running"#), "{own}");

    let retried = invoke("retryUntil3", &["\"retry\""]).await;
    assert!(retried.contains('3'), "{retried}");

    let joined = invoke("forkJoin", &[]).await;
    assert!(joined.contains("original+from-fork"), "{joined}");

    let reverted = invoke("revertOne", &["\"undo\""]).await;
    assert!(reverted.contains('1'), "{reverted}");

    let counters = invoke("counters", &[]).await;
    assert!(
        counters.contains(r#"CounterAgent("retry")"#)
            && counters.contains(r#"CounterAgent("undo")"#),
        "{counters}"
    );
    // The CLI's own log names the invoked agent; only the result line counts.
    let listed = counters
        .lines()
        .find(|line| line.contains("CounterAgent("))
        .unwrap_or_default();
    assert!(!listed.contains("OpsAgent"), "{counters}");

    let invocations = invoke("invocations", &[]).await;
    let n: i64 = invocations
        .lines()
        .rev()
        .find_map(|line| line.trim().parse().ok())
        .unwrap_or_else(|| panic!("no count in {invocations}"));
    assert!(n >= 6, "{invocations}");
}
