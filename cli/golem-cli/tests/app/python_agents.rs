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

//! These tests need the `golem-py-sdk` wheel, which `cargo make python-sdk-wheel`
//! builds; `cargo make cli-integration-tests-python` builds it and runs them.

use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use std::path::{Path, PathBuf};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

fn component_dir(ctx: &TestContext) -> PathBuf {
    std::fs::read_dir(ctx.cwd_path())
        .unwrap()
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .find(|path| path.join("pyproject.toml").exists())
        .unwrap_or_else(|| ctx.cwd_path().to_path_buf())
}

fn component_wasm(ctx: &TestContext) -> PathBuf {
    let agents = ctx.cwd_path_join("golem-temp/agents");
    let wasms = std::fs::read_dir(&agents)
        .unwrap_or_else(|err| panic!("cannot list {}: {err}", agents.display()))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "wasm"))
        .collect::<Vec<_>>();
    assert_eq!(wasms.len(), 1, "{wasms:?}");
    wasms.into_iter().next().unwrap()
}

fn edit(path: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains(from), "{} lacks {from:?}", path.display());
    std::fs::write(path, text.replacen(from, to, 1)).unwrap();
}

/// Builds the application and returns its output, failing the test when the
/// build fails.
async fn build(ctx: &TestContext) -> String {
    let outputs = ctx.cli([flag::YES, cmd::BUILD]).await;
    assert!(outputs.success_or_dump());
    format!("{}\n{}", outputs.stdout_text(), outputs.stderr_text())
}

/// Whether the build output reports that it skipped the Python assembly.
fn skipped_assembly(text: &str) -> bool {
    text.lines()
        .any(|line| line.contains("assembling Python modules in") && line.contains("UP-TO-DATE"))
}

/// Whether the build output reports that it ran the Python assembly.
fn ran_assembly(text: &str) -> bool {
    text.contains("Assembling Python modules in")
}

async fn invoke(ctx: &TestContext, agent: &str, method: &str, args: &[&str]) -> i64 {
    let mut argv = vec![flag::YES, cmd::AGENT, cmd::INVOKE, agent, method];
    argv.extend_from_slice(args);
    let outputs = ctx.cli(argv).await;
    assert!(outputs.success_or_dump());
    let stdout = outputs.stdout_text();
    stdout
        .lines()
        .rev()
        .find_map(|line| line.trim().parse().ok())
        .unwrap_or_else(|| panic!("no integer result in {stdout}"))
}

/// The `python` template builds, deploys and serves a durable counter. Its
/// state survives a restart of the server, an unchanged build is skipped and a
/// changed module is rebuilt and redeployed.
#[test]
#[timeout("20 minutes")]
async fn test_python_counter_builds_deploys_and_recovers() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    assert!(
        ctx.cli([flag::YES, cmd::NEW, "py-counter", flag::TEMPLATE, "python"])
            .await
            .success_or_dump()
    );
    ctx.cd("py-counter");
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());

    let agent = r#"CounterAgent("c")"#;
    assert_eq!(invoke(&ctx, agent, "increment", &[]).await, 1);
    assert_eq!(invoke(&ctx, agent, "add", &["7"]).await, 8);
    assert_eq!(invoke(&ctx, agent, "add", &["11"]).await, 19);
    // Another constructor argument identifies another agent with its own state.
    assert_eq!(invoke(&ctx, r#"CounterAgent("d")"#, "add", &["5"]).await, 5);

    // The executor restores the state from the agent's oplog after a restart.
    ctx.server_process.take().unwrap().kill().await.unwrap();
    ctx.startup_ports = None;
    ctx.start_server().await;
    assert_eq!(invoke(&ctx, agent, "value", &[]).await, 19);
    assert_eq!(invoke(&ctx, agent, "add", &["3"]).await, 22);

    // A build with unchanged inputs skips the assembly and keeps the component.
    let wasm = component_wasm(&ctx);
    let before = std::fs::read(&wasm).unwrap();
    let unchanged = ctx.cli([flag::YES, cmd::BUILD]).await;
    assert!(unchanged.success_or_dump());
    let text = format!("{}\n{}", unchanged.stdout_text(), unchanged.stderr_text());
    assert!(text.contains("assembling Python modules in"), "{text}");
    assert!(skipped_assembly(&text), "{text}");
    assert_eq!(std::fs::read(&wasm).unwrap(), before);

    // A changed module is assembled again, and the deployed agent runs it.
    let source = component_dir(&ctx).join("src/counter_agent.py");
    edit(
        &source,
        "    @method\n    def value(self) -> int:\n",
        "    @method\n    def doubled(self) -> int:\n        return 2 * self.count\n\n    @method\n    def value(self) -> int:\n",
    );
    let changed = ctx.cli([flag::YES, cmd::BUILD]).await;
    assert!(changed.success_or_dump());
    assert!(
        changed.stdout_text().contains("Assembling")
            || changed.stderr_text().contains("Assembling")
    );
    assert_ne!(std::fs::read(&wasm).unwrap(), before);
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());
    assert_eq!(invoke(&ctx, r#"CounterAgent("e")"#, "add", &["4"]).await, 4);
    assert_eq!(
        invoke(&ctx, r#"CounterAgent("e")"#, "doubled", &[]).await,
        8
    );
}

/// A Python build that fails reports the cause and its location, and keeps the
/// last component it assembled.
#[test]
#[timeout("10 minutes")]
async fn test_python_build_failures_keep_the_last_component() {
    let mut ctx = TestContext::new();
    assert!(
        ctx.cli([flag::YES, cmd::NEW, "py-fail", flag::TEMPLATE, "python"])
            .await
            .success_or_dump()
    );
    ctx.cd("py-fail");
    assert!(ctx.cli([flag::YES, cmd::BUILD]).await.success_or_dump());
    let wasm = component_wasm(&ctx);
    let valid = std::fs::read(&wasm).unwrap();

    let dir = component_dir(&ctx);
    let source = dir.join("src/counter_agent.py");
    let pyproject = dir.join("pyproject.toml");
    let original_source = std::fs::read_to_string(&source).unwrap();
    let original_pyproject = std::fs::read_to_string(&pyproject).unwrap();

    let cases: [(&str, &Path, &str, &str, &[&str]); 7] = [
        (
            "unsupported type hint",
            &source,
            "    def add(self, by: int) -> int:",
            "    def add(self, by: list[int]) -> int:",
            &[
                "counter_agent.py:",
                "parameter `by` of method `CounterAgent.add` has the unsupported type `list[int]`",
            ],
        ),
        (
            "import error",
            &source,
            "from golem import agent, method\n",
            "from golem import agent, method\n\nraise ValueError('cannot load settings')\n",
            &[
                "ValueError: cannot load settings",
                "counter_agent.py\", line 3",
            ],
        ),
        (
            "third-party dependency",
            &pyproject,
            "dependencies = [\"golem-py-sdk==",
            "dependencies = [\"requests>=2\", \"golem-py-sdk==",
            &[
                "depends on `requests>=2`",
                "cannot contain third-party packages",
            ],
        ),
        (
            "SDK path source",
            &pyproject,
            "[project]\n",
            "[tool.uv.sources]\ngolem-py-sdk = { path = \"../patched-sdk\" }\n\n[project]\n",
            &[
                "declares the package source `tool.uv.sources.golem-py-sdk`, but",
                "set GOLEM_PYTHON_SDK_WHEEL",
            ],
        ),
        (
            "SDK Git source",
            &pyproject,
            "[project]\n",
            "[tool.uv.sources]\ngolem-py-sdk = { git = \"https://TOKENSENTINEL@example.invalid/sdk.git\" }\n\n[project]\n",
            &[
                "declares the package source `tool.uv.sources.golem-py-sdk`, but",
                "set GOLEM_PYTHON_SDK_WHEEL",
            ],
        ),
        (
            "direct reference",
            &pyproject,
            "dependencies = [\"golem-py-sdk==",
            "dependencies = [\"requests @ https://USERSENTINEL:PASSSENTINEL@example.invalid/r.whl\", \"golem-py-sdk==",
            &[
                "depends on `requests @ <URL omitted>`",
                "cannot contain third-party packages",
            ],
        ),
        (
            "authenticated index",
            &pyproject,
            "[project]\n",
            "[[tool.uv.index]]\nname = \"private\"\nurl = \"https://USERSENTINEL:PASSSENTINEL@packages.example.invalid/simple\"\n\n[project]\n",
            &[
                "declares the package source `tool.uv.index`, but",
                "set GOLEM_PYTHON_SDK_WHEEL",
            ],
        ),
    ];
    for (case, path, from, to, expected) in cases {
        edit(path, from, to);
        let failed = ctx.cli([flag::YES, cmd::BUILD]).await;
        let text = format!("{}\n{}", failed.stdout_text(), failed.stderr_text());
        assert!(!failed.success(), "{case}: the build succeeded: {text}");
        // Package source values can contain credentials, so the output omits them.
        for output in [failed.stdout_text(), failed.stderr_text()] {
            for value in [
                "patched-sdk",
                "example.invalid",
                "TOKENSENTINEL",
                "USERSENTINEL",
                "PASSSENTINEL",
            ] {
                assert!(!output.contains(value), "{case}: {value:?} is in {output}");
            }
        }
        for expected in expected {
            assert!(
                text.contains(expected),
                "{case}: {expected:?} is missing in {text}"
            );
        }
        assert_eq!(
            std::fs::read(&wasm).unwrap(),
            valid,
            "{case}: the failed build replaced the component"
        );
        std::fs::write(&source, &original_source).unwrap();
        std::fs::write(&pyproject, &original_pyproject).unwrap();
    }
}

/// Removing a module is a change: the build assembles again, so a module that
/// imports the removed one fails the build, which keeps the last component.
#[test]
#[timeout("10 minutes")]
async fn test_python_build_follows_removed_source_files() {
    let mut ctx = TestContext::new();
    assert!(
        ctx.cli([flag::YES, cmd::NEW, "py-removed", flag::TEMPLATE, "python"])
            .await
            .success_or_dump()
    );
    ctx.cd("py-removed");
    let src = component_dir(&ctx).join("src");
    let helper = src.join("helper.py");
    std::fs::write(&helper, "LABEL = 'helper'\n").unwrap();
    edit(
        &src.join("main.py"),
        "import counter_agent  # noqa: F401\n",
        "import counter_agent  # noqa: F401\nimport helper  # noqa: F401\n",
    );
    build(&ctx).await;
    let wasm = component_wasm(&ctx);
    let valid = std::fs::read(&wasm).unwrap();

    let unchanged = build(&ctx).await;
    assert!(skipped_assembly(&unchanged), "{unchanged}");
    assert_eq!(std::fs::read(&wasm).unwrap(), valid);

    std::fs::remove_file(&helper).unwrap();
    let failed = ctx.cli([flag::YES, cmd::BUILD]).await;
    let text = format!("{}\n{}", failed.stdout_text(), failed.stderr_text());
    assert!(
        !failed.success(),
        "the build after removing helper.py succeeded: {text}"
    );
    assert!(text.contains("No module named 'helper'"), "{text}");
    assert_eq!(
        std::fs::read(&wasm).unwrap(),
        valid,
        "the failed build replaced the component"
    );

    std::fs::write(&helper, "LABEL = 'helper'\n").unwrap();
    let restored = build(&ctx).await;
    assert!(ran_assembly(&restored), "{restored}");
    let unchanged = build(&ctx).await;
    assert!(skipped_assembly(&unchanged), "{unchanged}");
}

const ENTRY_MODULE_MANIFEST: &str = r#"
manifestVersion: MANIFEST_VERSION
app: py-entries
environments:
  local:
    server: local
componentTemplates:
  py:
    guestLanguage: python
    presets:
      default:
        default: true
        build:
        - assemblePython: "{{ componentDir }}/src"
          modules:
          - ENTRY_MODULE
          pyproject: "{{ componentDir }}/pyproject.toml"
          into: "{{ golemTempDir }}/agents/{{ component_name | to_snake_case }}.wasm"
        componentWasm: "{{ golemTempDir }}/agents/{{ component_name | to_snake_case }}.wasm"
components:
  py-entries:main:
    dir: "COMPONENT_DIR"
    templates: py
"#;

/// Builds with other entry modules write the same output. Switching back to
/// the first entry module assembles it again instead of keeping the
/// component of the second.
#[test]
#[timeout("10 minutes")]
async fn test_python_build_follows_entry_module_switches() {
    let mut ctx = TestContext::new();
    assert!(
        ctx.cli([flag::YES, cmd::NEW, "py-entries", flag::TEMPLATE, "python"])
            .await
            .success_or_dump()
    );
    ctx.cd("py-entries");
    let component = component_dir(&ctx);
    let src = component.join("src");
    std::fs::write(
        src.join("entry_a.py"),
        "import counter_agent  # noqa: F401\n",
    )
    .unwrap();
    std::fs::write(
        src.join("entry_b.py"),
        "from golem import agent, method\n\n\n@agent\nclass GreeterAgent:\n    def __init__(self, name: str) -> None:\n        self.name = name\n\n    @method\n    def greet(self) -> str:\n        return 'hello ' + self.name\n",
    )
    .unwrap();
    let component_dir = component
        .strip_prefix(ctx.cwd_path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let configure = |entry: &str| {
        std::fs::write(
            ctx.cwd_path_join("golem.yaml"),
            ENTRY_MODULE_MANIFEST
                .replace("MANIFEST_VERSION", golem_cli::versions::sdk::MANIFEST)
                .replace("ENTRY_MODULE", entry)
                .replace("COMPONENT_DIR", &component_dir),
        )
        .unwrap();
    };

    configure("entry_a");
    build(&ctx).await;
    let wasm = component_wasm(&ctx);
    let a = std::fs::read(&wasm).unwrap();

    configure("entry_b");
    let switched = build(&ctx).await;
    assert!(ran_assembly(&switched), "{switched}");
    let b = std::fs::read(&wasm).unwrap();
    assert_ne!(a, b, "entry_a and entry_b declare different agents");

    configure("entry_a");
    let back = build(&ctx).await;
    assert!(ran_assembly(&back), "{back}");
    assert_eq!(
        std::fs::read(&wasm).unwrap(),
        a,
        "switching back to entry_a kept the entry_b component"
    );
    let unchanged = build(&ctx).await;
    assert!(skipped_assembly(&unchanged), "{unchanged}");
    assert_eq!(std::fs::read(&wasm).unwrap(), a);
}
