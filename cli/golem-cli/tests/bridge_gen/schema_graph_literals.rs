// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use crate::bridge_gen::schema_graph_test_fixture::exhaustive_schema_graph;
use golem_cli::sdk_overrides::workspace_root;
use std::process::{Command, Output};
use tempfile::{Builder, TempDir};
use test_r::test;

mod rust_emitter {
    include!("../../src/bridge_gen/rust/schema_graph.rs");
}

mod typescript_emitter {
    include!("../../src/bridge_gen/typescript/schema_graph.rs");
}

mod scala_emitter {
    fn scala_string_literal(value: &str) -> String {
        let mut escaped = String::with_capacity(value.len() + 2);
        escaped.push('"');
        for ch in value.chars() {
            match ch {
                '\\' => escaped.push_str("\\\\"),
                '"' => escaped.push_str("\\\""),
                '\n' => escaped.push_str("\\n"),
                '\r' => escaped.push_str("\\r"),
                '\t' => escaped.push_str("\\t"),
                other if (other as u32) < 0x20 => {
                    escaped.push_str(&format!("\\u{:04x}", other as u32))
                }
                other => escaped.push(other),
            }
        }
        escaped.push('"');
        escaped
    }

    pub(crate) mod schema_graph {
        include!("../../src/bridge_gen/scala/schema_graph.rs");
    }
}

fn assert_success(context: &str, output: Output) {
    assert!(
        output.status.success(),
        "{context} failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

pub(super) fn canonical_carrier_shape() -> (usize, i32, Vec<(String, i32)>) {
    let wire = golem_common::schema::wit::encode_graph(&exhaustive_schema_graph()).unwrap();
    (
        wire.type_nodes.len(),
        wire.root,
        wire.defs
            .into_iter()
            .map(|def| (def.id, def.body))
            .collect(),
    )
}

#[test]
fn exhaustive_rust_literal_compiles_executes_and_round_trips_through_wit() {
    let dir = TempDir::new().unwrap();
    let workspace = workspace_root().unwrap();
    let graph = exhaustive_schema_graph();
    let literal = rust_emitter::emit_schema_graph_literal(&graph);
    let (node_count, root, defs) = canonical_carrier_shape();
    let def_assertions = defs.iter().map(|(id, body)| {
        format!("assert!(wire.defs.iter().any(|def| def.id == {id:?} && def.body == {body}));")
    });

    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        format!(
            "[package]\nname = \"schema-graph-literal-check\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\ngolem-rust = {{ path = {:?} }}\n",
            workspace.join("sdks/rust/golem-rust")
        ),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("src/main.rs"),
        format!(
            "fn main() {{\n  let graph: golem_rust::SchemaGraph = {literal};\n  let wire = golem_rust::encode_schema_graph(&graph).unwrap();\n  assert_eq!(wire.type_nodes.len(), {node_count});\n  assert_eq!(wire.root, {root});\n  assert_eq!(wire.defs.len(), {});\n  {}\n  assert_eq!(golem_rust::decode_schema_graph(&wire).unwrap(), graph);\n}}\n",
            defs.len(),
            def_assertions.collect::<Vec<_>>().join("\n  ")
        ),
    )
    .unwrap();

    assert_success(
        "exhaustive Rust schema graph literal",
        Command::new("cargo")
            .arg("run")
            .arg("--quiet")
            .current_dir(dir.path())
            .output()
            .unwrap(),
    );
}

#[test]
fn exhaustive_typescript_literal_compiles_executes_and_round_trips_through_wit() {
    let workspace = workspace_root().unwrap();
    let sdk_package = workspace.join("sdks/ts/packages/golem-ts-sdk");
    let dir = Builder::new()
        .prefix("schema-graph-literal-")
        .tempdir_in(sdk_package.join("tests"))
        .unwrap();
    let graph = exhaustive_schema_graph();
    let literal = typescript_emitter::emit_schema_graph_literal(&graph);
    let (node_count, root, defs) = canonical_carrier_shape();
    let defs = serde_json::to_string(&defs).unwrap();
    let schema_model =
        workspace.join("sdks/ts/packages/golem-ts-sdk/src/internal/schema-model/index.ts");
    std::fs::write(
        dir.path().join("literal.test.ts"),
        format!(
            r#"import {{ test }} from 'vitest';
import * as base from {schema_model:?};

test('exhaustive native schema graph matches the canonical WIT carrier shape', () => {{
  const graph: base.SchemaGraph = {literal};
  const wire = base.schemaGraphToWit(graph);
  const expectedDefs: [string, number][] = {defs};
  if (wire.typeNodes.length !== {node_count}) throw new Error('type-node count differs from canonical Rust carrier');
  if (wire.root !== {root}) throw new Error('root index differs from canonical Rust carrier');
  if (wire.defs.length !== expectedDefs.length) throw new Error('definition count differs from canonical Rust carrier');
  for (const [id, body] of expectedDefs) {{
    if (!wire.defs.some((def) => def.id === id && def.body === body)) throw new Error(`missing canonical definition ${{id}}`);
  }}
  if (!base.deepEqual(base.schemaGraphFromWit(wire), graph)) throw new Error('WIT round-trip changed emitted graph');
}});
"#,
        ),
    )
    .unwrap();

    assert_success(
        "exhaustive TypeScript schema graph literal",
        Command::new("npx")
            .arg("pnpm@10.17.1")
            .arg("--filter")
            .arg("@golemcloud/golem-ts-sdk")
            .arg("exec")
            .arg("vitest")
            .arg("run")
            .arg(dir.path().join("literal.test.ts"))
            .current_dir(workspace.join("sdks/ts"))
            .output()
            .unwrap(),
    );
}

#[test]
fn exhaustive_scala_literal_compiles_executes_and_round_trips_through_wit() {
    let dir = TempDir::new().unwrap();
    let graph = exhaustive_schema_graph();
    let literal = scala_emitter::schema_graph::emit_schema_graph_literal(&graph);
    let (node_count, root, defs) = canonical_carrier_shape();
    let def_assertions = defs.iter().map(|(id, body)| {
        format!(
            "assert(wire.defs.exists(defn => defn.id == {:?} && defn.body == {body}))",
            id
        )
    });
    std::fs::create_dir_all(dir.path().join("src/main/scala")).unwrap();
    std::fs::create_dir(dir.path().join("project")).unwrap();
    std::fs::write(
        dir.path().join("project/build.properties"),
        "sbt.version=1.12.1\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("build.sbt"),
        "ThisBuild / scalaVersion := \"3.8.2\"\nscalacOptions += \"-experimental\"\nlibraryDependencies += \"cloud.golem\" %% \"golem-scala-model\" % \"0.0.0-SNAPSHOT\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("src/main/scala/Main.scala"),
        format!(
            "@main def checkSchemaGraphLiteral(): Unit = {{\n  val graph: _root_.golem.schema.SchemaGraph = {literal}\n  val wire = _root_.golem.schema.wire.SchemaWire.schemaGraphToWit(graph)\n  assert(wire.typeNodes.length == {node_count})\n  assert(wire.root == {root})\n  assert(wire.defs.length == {})\n  {}\n  assert(_root_.golem.schema.wire.SchemaWire.schemaGraphFromWit(wire) == graph)\n}}\n",
            defs.len(),
            def_assertions.collect::<Vec<_>>().join("\n  ")
        ),
    )
    .unwrap();

    assert_success(
        "exhaustive Scala schema graph literal",
        Command::new("sbt")
            .arg("-Dsbt.color=false")
            .arg("--batch")
            .arg("run")
            .current_dir(dir.path())
            .output()
            .unwrap(),
    );
}
