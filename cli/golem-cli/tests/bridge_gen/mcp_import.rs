// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use camino::Utf8Path;
use golem_cli::bridge_gen::effect::effect_tool::EffectToolBridgeGenerator;
use golem_cli::bridge_gen::moonbit::tool::MoonBitToolBridgeGenerator;
use golem_cli::bridge_gen::rust::tool::RustToolBridgeGenerator;
use golem_cli::bridge_gen::typescript::tool::TypeScriptToolBridgeGenerator;
use golem_mcp_import::tool::{Limits, ProjectedTool};
use serde_json::json;
use tempfile::TempDir;
use test_r::{tag, test};

pub(super) fn projections() -> (ProjectedTool, ProjectedTool) {
    let input = json!({
        "type": "object",
        "properties": {
            "query": { "type": "string" },
            "limit": { "type": "integer" }
        },
        "required": ["query"],
        "additionalProperties": { "type": "string" }
    });
    let typed = ProjectedTool::new(
        &json!({
            "name": "typed_lookup",
            "inputSchema": input,
            "outputSchema": {
                "type": "object",
                "properties": { "answer": { "type": "string" }, "score": { "type": "integer" } },
                "required": ["answer", "score"],
                "additionalProperties": false
            }
        }),
        "typed-lookup",
        Limits::default(),
    )
    .unwrap();
    let mixed = ProjectedTool::new(
        &json!({ "name": "mixed_lookup", "inputSchema": input }),
        "mixed-lookup",
        Limits::default(),
    )
    .unwrap();
    (typed, mixed)
}

fn run(command: &str, args: &[&str], dir: &Utf8Path) {
    let output = std::process::Command::new(command)
        .args(args)
        .current_dir(dir)
        .env("CARGO_INCREMENTAL", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{command} {} failed in {dir}\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn append(path: &Utf8Path, consumer: &str) {
    let source = std::fs::read_to_string(path).unwrap();
    std::fs::write(path, format!("{source}\n{consumer}")).unwrap();
}

#[test]
#[tag(bridge_gen_rust)]
fn mcp_projections_compile_with_rust_generator() {
    for (projected, typed) in [(projections().0, true), (projections().1, false)] {
        let dir = TempDir::new().unwrap();
        let target = Utf8Path::from_path(dir.path()).unwrap();
        RustToolBridgeGenerator::new(projected.definition, target, true)
            .unwrap()
            .generate()
            .unwrap();
        let (client, method, fields) = if typed {
            (
                "TypedLookupClient",
                "typed_lookup",
                "let _: String = result.structured.answer; let _: i64 = result.structured.score;",
            )
        } else {
            (
                "MixedLookupClient",
                "mixed_lookup",
                "let _: Option<String> = result.structured;",
            )
        };
        append(
            &target.join("src/lib.rs"),
            &format!(
                r#"
async fn consume(client: &{client}) {{
    let invocation = client.{method}(None, "query".into(), vec![]).await.unwrap();
    let _ = client.{method}(Some(3), "query".into(), vec![("region".into(), "west".into())]).await.unwrap();
    let result = invocation.result().await.unwrap();
    {fields}
    let _ = result.content;
    let _: golem_rust::agentic::ToolInvocationStdout = invocation.stdout;
}}
"#
            ),
        );
        let shared_target = crate::workspace_path().join("target/shared_bridge_tests");
        run(
            "cargo",
            &["check", "--target-dir", shared_target.to_str().unwrap()],
            target,
        );
    }
}

#[test]
fn mcp_projections_compile_with_typescript_generator() {
    for (projected, typed) in [(projections().0, true), (projections().1, false)] {
        let dir = TempDir::new().unwrap();
        let target = Utf8Path::from_path(dir.path()).unwrap();
        TypeScriptToolBridgeGenerator::new(projected.definition, target, true)
            .unwrap()
            .generate()
            .unwrap();
        let (name, client, fields) = if typed {
            (
                "typed",
                "TypedLookupClient",
                "const answer: string = result.structured.answer; const score: bigint = result.structured.score; void answer; void score;",
            )
        } else {
            (
                "mixed",
                "MixedLookupClient",
                "const structured: string | undefined = result.structured; void structured;",
            )
        };
        append(
            &target.join(format!("{name}-lookup-tool-guest-client.ts")),
            &format!(
                r#"
async function consume(client: {client}) {{
    const invocation = client.{name}_lookup(undefined, "query", new Map());
    void client.{name}_lookup(3n, "query", new Map([["region", "west"]]));
    const stdout: ReadableStream<Uint8Array> = invocation.stdout;
    const result = await invocation.result;
    {fields}
    void result.content; void stdout;
}}
void consume;
"#
            ),
        );
        run("npm", &["install"], target);
        run("npm", &["run", "build"], target);
    }
}

#[test]
fn mcp_projections_compile_with_effect_generator() {
    for (projected, typed) in [(projections().0, true), (projections().1, false)] {
        let dir = TempDir::new().unwrap();
        let target = Utf8Path::from_path(dir.path()).unwrap();
        EffectToolBridgeGenerator::new(projected.definition, target, true)
            .unwrap()
            .generate()
            .unwrap();
        let (name, client, fields) = if typed {
            (
                "typed",
                "TypedLookupClient",
                "const answer: string = result.structured.answer; const score: bigint = result.structured.score; void answer; void score;",
            )
        } else {
            (
                "mixed",
                "MixedLookupClient",
                "const structured: string | undefined = result.structured; void structured;",
            )
        };
        append(
            &target.join(format!("{name}-lookup-tool-guest-client.ts")),
            &format!(
                r#"
function consume(client: {client}) {{
    return Effect.gen(function*() {{
        const invocation = yield* client.{name}_lookup(undefined, "query", new Map());
        yield* client.{name}_lookup(3n, "query", new Map([["region", "west"]]));
        const result = yield* invocation.result;
        {fields}
        void result.content; void invocation.stdout;
    }});
}}
void consume;
"#
            ),
        );
        run("npm", &["install"], target);
        run("npm", &["run", "build"], target);
    }
}

#[test]
fn mcp_projections_compile_with_moonbit_generator() {
    for (projected, typed) in [(projections().0, true), (projections().1, false)] {
        let dir = TempDir::new().unwrap();
        let target = Utf8Path::from_path(dir.path()).unwrap();
        MoonBitToolBridgeGenerator::new(projected.definition, target, true)
            .unwrap()
            .generate()
            .unwrap();
        let (name, client, fields) = if typed {
            (
                "typed",
                "TypedLookupClient",
                "let _ : String = result.structured.answer\nlet _ : Int64 = result.structured.score",
            )
        } else {
            (
                "mixed",
                "MixedLookupClient",
                "let _ : String? = result.structured",
            )
        };
        std::fs::write(
            target.join("client/consumer.mbt"),
            format!(
                r#"
///|
pub async fn consume(client : {client}) -> Unit {{
  ignore(client.{name}_lookup(None, "query", {{}}))
  match client.{name}_lookup(Some(3L), "query", {{ "region": "west" }}) {{
    Err(_) => ()
    Ok(invocation) => {{
      ignore(invocation.stdout)
      match invocation.get() {{
        Err(_) => ()
        Ok(result) => {{
          {fields}
          ignore(result.content)
        }}
      }}
    }}
  }}
}}
"#
            ),
        )
        .unwrap();
        run(
            "moon",
            &["check", "--target", "wasm", "--deny-warn"],
            target,
        );
    }
}
