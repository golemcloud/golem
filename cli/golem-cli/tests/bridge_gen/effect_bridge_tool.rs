// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use crate::bridge_gen::scala::grep_tool;
use camino::Utf8Path;
use golem_cli::bridge_gen::typescript::effect_tool::EffectToolBridgeGenerator;
use golem_common::schema::tool::{CommandIndex, StreamSpec};
use tempfile::TempDir;
use test_r::test;

#[test]
fn effect_tool_advanced_schema_inherited_globals_repeatables_and_streams_compile() {
    let mut tool = grep_tool();
    let body = tool.commands.nodes[0].body.as_mut().unwrap();
    body.stdin = Some(StreamSpec {
        doc: Default::default(),
        mime: vec![],
        required: false,
    });
    body.stdout = Some(StreamSpec {
        doc: Default::default(),
        mime: vec![],
        required: true,
    });
    let mut nested_leaf = tool.commands.nodes[1].clone();
    nested_leaf.name = "run".into();
    tool.commands.nodes[1].body = None;
    tool.commands.nodes[1].subcommands = vec![CommandIndex(2)];
    tool.commands.nodes.push(nested_leaf);
    let dir = TempDir::new().unwrap();
    let target = Utf8Path::from_path(dir.path()).unwrap();
    EffectToolBridgeGenerator::new(tool, target, true)
        .unwrap()
        .generate()
        .unwrap();
    let generated = target.join("grep-tool-guest-client.ts");
    let source = std::fs::read_to_string(&generated).unwrap();
    for expected in [
        "import { BridgeTool as base } from '@golemcloud/effect-golem'",
        "const __golemSchemaGraphs = {",
        "base.createToolClientRuntime(\"grep\")",
        "inherited.map(encode => encode())",
        "const typedInput: base.TypedSchemaValue = yield* Effect.try",
        "export const client = base.client",
        "Effect.Effect<",
        "base.StartedToolInvocation<",
        "base.ToolInputStream<",
        "() => (encodeColorMode(color))",
        "() => ({ tag: 'bool', value: case_sensitive })",
    ] {
        assert!(
            source.contains(expected),
            "missing {expected} in {generated}"
        );
    }
    assert!(
        source.contains("[...this.inherited, () => (encodeColorMode(color)), () => ({ tag: 'bool', value: case_sensitive })]"),
        "inherited global encoder expressions were not preserved as complete array elements in {generated}"
    );
    assert!(!source.contains("Schema.Top"));
    let consumer = r#"import { Effect, Stream } from "effect"
import { client } from "./grep-tool-guest-client.js"
const grep = client.grep("auto", false, "needle", [], 0, 10, Stream.empty)
const replace = client.replace("auto", false).run("needle", "replacement")
const checkedGrep: Effect.Effect<unknown, unknown, unknown> = grep
const checkedReplace: Effect.Effect<unknown, unknown, unknown> = replace
void checkedGrep
void checkedReplace
"#;
    std::fs::write(target.join("consumer.ts"), consumer).unwrap();
    let config = target.join("tsconfig.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    json["include"]
        .as_array_mut()
        .unwrap()
        .push("consumer.ts".into());
    std::fs::write(&config, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    {
        let args = &["install"][..];
        let output = std::process::Command::new("npm")
            .args(args)
            .current_dir(target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "npm {} failed for {generated}\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = std::process::Command::new("npm")
        .args(["run", "build"])
        .current_dir(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "npm run build failed for {generated}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
