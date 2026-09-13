// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use crate::bridge_gen::fixtures::{agent, field, guest_streaming_agent_type, local_config, method};
use camino::Utf8Path;
use golem_cli::bridge_gen::BridgeGenerator;
use golem_cli::bridge_gen::typescript::effect_guest::EffectGuestBridgeGenerator;
use golem_common::model::agent::AgentMode;
use golem_common::schema::SchemaType;
use tempfile::TempDir;
use test_r::test;

#[test]
fn effect_guest_all_schema_nested_streams_and_durable_config_compile() {
    let mut schema = guest_streaming_agent_type("effect");
    schema.config = vec![local_config(vec!["limits", "maximum"], SchemaType::u64())];
    generate_and_compile(
        schema,
        "guest-streaming-agent-guest-client.ts",
        r#"
import { Effect } from "effect"
import { GuestStreamingAgent } from "./guest-streaming-agent-guest-client.js"
const client = GuestStreamingAgent.getWithConfig("id", 10n)
const call = Effect.flatMap(client, value => value.status())
void call
"#,
    );
}

#[test]
fn effect_guest_ephemeral_metadata_and_config_compile() {
    let mut schema = agent(
        "EphemeralEffectAgent",
        "effect",
        vec![field("name", SchemaType::string())],
        vec![method(
            "run",
            vec![field("count", SchemaType::u32())],
            Some(SchemaType::string()),
        )],
        vec![],
        AgentMode::Ephemeral,
    );
    schema.config = vec![local_config(vec!["model"], SchemaType::string())];
    generate_and_compile(
        schema,
        "ephemeral-effect-agent-guest-client.ts",
        r#"
import { Effect } from "effect"
import { EphemeralEffectAgent } from "./ephemeral-effect-agent-guest-client.js"
const result = Effect.flatMap(EphemeralEffectAgent.newPhantomWithConfig("name", "model"), client => client.run(1))
const checked: Effect.Effect<{ readonly metadata: unknown; readonly value: string }, unknown, unknown> = result
void checked
"#,
    );
}

fn generate_and_compile(
    schema: golem_common::schema::AgentTypeSchema,
    filename: &str,
    consumer: &str,
) {
    let dir = TempDir::new().unwrap();
    let target = Utf8Path::from_path(dir.path()).unwrap();
    EffectGuestBridgeGenerator::new(schema, target, true)
        .unwrap()
        .generate()
        .unwrap();
    let generated = target.join(filename);
    let source = std::fs::read_to_string(&generated).unwrap();
    assert!(
        source.contains("Effect.Effect<"),
        "missing typed Effect methods in {generated}"
    );
    assert!(
        !source.contains("Effect.Effect<any"),
        "untyped Effect method in {generated}"
    );
    std::fs::write(target.join("consumer.ts"), consumer).unwrap();
    let config = target.join("tsconfig.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    json["include"]
        .as_array_mut()
        .unwrap()
        .push("consumer.ts".into());
    std::fs::write(&config, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    for args in [&["install"][..], &["run", "build"][..]] {
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
}
