// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use crate::bridge_gen::fixtures::{
    agent, def, field, guest_streaming_agent_type, local_config, method, named_field, ref_to,
};
use camino::Utf8Path;
use golem_cli::bridge_gen::BridgeGenerator;
use golem_cli::bridge_gen::typescript::effect_external::EffectExternalBridgeGenerator;
use golem_common::model::agent::AgentMode;
use golem_common::schema::SchemaType;
use tempfile::TempDir;
use test_r::test;

#[test]
fn effect_external_all_schema_streaming_consumer_compiles() {
    let mut schema = guest_streaming_agent_type("rust");
    schema.config = vec![local_config(vec!["limits", "maximum"], SchemaType::u64())];
    schema.schema.defs.push(def(
        "MappedInput",
        SchemaType::record(vec![named_field("item_stream", SchemaType::string())]),
    ));
    schema.methods.push(method(
        "rename",
        vec![field("payload", ref_to("MappedInput"))],
        None,
    ));
    schema
        .methods
        .iter_mut()
        .find(|method| method.name == "status")
        .unwrap()
        .output_schema =
        golem_common::schema::agent::OutputSchema::Single(Box::new(ref_to("StreamItem")));
    let stream_item = schema
        .schema
        .defs
        .iter_mut()
        .find(|definition| definition.id.0 == "StreamItem")
        .unwrap();
    if let SchemaType::Record { fields, .. } = &mut stream_item.body {
        fields[0].name = "item_stream".into();
    }
    let dir = TempDir::new().unwrap();
    let target = Utf8Path::from_path(dir.path()).unwrap();
    EffectExternalBridgeGenerator::new(schema, target, true)
        .unwrap()
        .generate()
        .unwrap();

    let generated = target.join("guest-streaming-agent-client.ts");
    let source = std::fs::read_to_string(&generated).unwrap();
    assert!(
        source.contains("Effect.Effect<"),
        "missing typed Effect methods in {generated}"
    );
    assert!(
        source.contains("Stream.Stream<"),
        "missing Effect streams in {generated}"
    );
    assert!(
        !source.contains("Effect.Effect<any"),
        "untyped Effect method in {generated}"
    );
    std::fs::write(
        target.join("consumer.ts"),
        r#"import { Effect, Fiber, Scope, Stream } from "effect"
import { GuestStreamingAgent, type StreamItem } from "./guest-streaming-agent-client.js"

function compileChecks(client: GuestStreamingAgent) {
  const produced: Effect.Effect<Stream.Stream<StreamItem, unknown>, unknown, Scope.Scope> = client.produce()
  const nested: Effect.Effect<Stream.Stream<Stream.Stream<StreamItem, unknown>, unknown>, unknown, Scope.Scope> =
    client.nested(Stream.make(Stream.make({ itemStream: "root", children: [] })))
  const status: Effect.Effect<StreamItem, unknown> = client.status()
  const configured = GuestStreamingAgent.getWithConfig("id", 10n)
  void produced
  void nested
  void status
  void configured
}
void compileChecks
// The underlying generated transport takes TypeScript member names, not wire field names.
let triggered: unknown
const raw = {
  rename: {
    abortable: async () => undefined,
    trigger: (payload: unknown) => { triggered = payload },
    schedule: () => undefined,
  },
}
const mapped = new (GuestStreamingAgent as any)(raw) as GuestStreamingAgent
await Effect.runPromise(mapped.renametrigger({ itemStream: "kept" }))
if (JSON.stringify(triggered) !== JSON.stringify({ itemStream: "kept" })) {
  throw new Error(`sync variant lost mapped record member: ${JSON.stringify(triggered)}`)
}

let invocationSignal: AbortSignal | undefined
let invocationStarted!: () => void
const started = new Promise<void>(resolve => { invocationStarted = resolve })
const pendingRaw = {
  produce: {
    abortable: (signal: AbortSignal) => {
      invocationSignal = signal
      invocationStarted()
      return new Promise(() => undefined)
    },
  },
}
const pending = new (GuestStreamingAgent as any)(pendingRaw) as GuestStreamingAgent
await Effect.runPromise(Effect.scoped(Effect.gen(function*() {
  const fiber = yield* Effect.forkChild(pending.produce())
  yield* Effect.promise(() => started)
  if (invocationSignal?.aborted) throw new Error("invocation aborted before interruption")
  yield* Fiber.interrupt(fiber)
  if (!invocationSignal?.aborted) throw new Error("interruption did not abort pending invocation")
})))
"#,
    )
    .unwrap();
    compile(target, &generated);
    let output = std::process::Command::new("node")
        .arg("consumer.js")
        .current_dir(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "generated consumer failed for {generated}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn effect_external_nonstream_named_record_consumer_compiles() {
    let schema = agent(
        "NamedRecordAgent",
        "rust",
        vec![field("profile", ref_to("Profile"))],
        vec![method(
            "replace",
            vec![field("profile", ref_to("Profile"))],
            Some(ref_to("Profile")),
        )],
        vec![def(
            "Profile",
            SchemaType::record(vec![named_field("display_name", SchemaType::string())]),
        )],
        AgentMode::Durable,
    );
    let dir = TempDir::new().unwrap();
    let target = Utf8Path::from_path(dir.path()).unwrap();
    EffectExternalBridgeGenerator::new(schema, target, true)
        .unwrap()
        .generate()
        .unwrap();

    let generated = target.join("named-record-agent-client.ts");
    std::fs::write(
        target.join("consumer.ts"),
        r#"import { Effect } from "effect"
import { NamedRecordAgent, type Profile } from "./named-record-agent-client.js"

const profile: Profile = { displayName: "Ada" }
const created: Effect.Effect<NamedRecordAgent, unknown> = NamedRecordAgent.get(profile)
declare const client: NamedRecordAgent
const replaced: Effect.Effect<Profile, unknown> = client.replace(profile)
void created
void replaced
"#,
    )
    .unwrap();
    compile(target, &generated);
}

#[test]
fn effect_external_identifiers_do_not_shadow_arguments_or_rewrite_methods() {
    let params = || {
        [
            "value",
            "raw",
            "controller",
            "signal",
            "__arg0",
            "at",
            "Effect",
            "__schema",
        ]
        .into_iter()
        .map(|name| field(name, SchemaType::string()))
        .collect()
    };
    let schema = agent(
        "CollisionAgent",
        "typescript",
        params(),
        vec![
            method("echo", params(), Some(SchemaType::string())),
            method(
                "signalEvents",
                params(),
                Some(SchemaType::stream(Some(SchemaType::string()))),
            ),
        ],
        vec![],
        AgentMode::Durable,
    );
    let dir = TempDir::new().unwrap();
    let target = Utf8Path::from_path(dir.path()).unwrap();
    EffectExternalBridgeGenerator::new(schema, target, true)
        .unwrap()
        .generate()
        .unwrap();
    let generated = target.join("collision-agent-client.ts");
    std::fs::write(target.join("consumer.ts"), r#"import { Effect, Stream } from "effect"
import { CollisionAgent } from "./collision-agent-client.js"
import { CollisionAgent as Transport } from "./internal/transport/collision-agent-client.js"

const args = ["v", "r", "c", "s", "a", "t", "e", "schema"] as const
const check = (received: unknown[]) => {
  if (JSON.stringify(received) !== JSON.stringify(args)) throw new Error(`wrong arguments: ${JSON.stringify(received)}`)
}
let streamSignal: AbortSignal | undefined
const raw = {
  echo: {
    abortable: async (_signal: AbortSignal, ...received: unknown[]) => { check(received); return "echoed" },
    trigger: (...received: unknown[]) => check(received),
    schedule: (at: string, ...received: unknown[]) => { if (at !== "later") throw new Error(at); check(received) },
  },
  signalEvents: {
    abortable: async (signal: AbortSignal, ...received: unknown[]) => {
      check(received)
      streamSignal = signal
      return (async function*() { yield "event" })()
    },
  },
}
;(Transport as any).get = async (...received: unknown[]) => { check(received); return raw }
await Effect.runPromise(Effect.scoped(Effect.gen(function*() {
  const client = yield* CollisionAgent.get(...args)
  if ((yield* client.echo(...args)) !== "echoed") throw new Error("wrong echo")
  yield* client.echotrigger(...args)
  yield* client.echoschedule("later", ...args)
  const stream = yield* client.signalEvents(...args)
  if (streamSignal?.aborted) throw new Error("successful stream aborted before consumption")
  const collected = yield* Stream.runCollect(stream)
  if (JSON.stringify(Array.from(collected)) !== '["event"]') throw new Error("wrong stream")
})))
if (!streamSignal?.aborted) throw new Error("scope did not abort stream session")
"#).unwrap();
    compile(target, &generated);
    let output = std::process::Command::new("node")
        .arg("consumer.js")
        .current_dir(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn compile(target: &Utf8Path, generated: &Utf8Path) {
    let config = target.join("tsconfig.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    json["include"]
        .as_array_mut()
        .unwrap()
        .push("consumer.ts".into());
    std::fs::write(&config, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    run(target, "install", generated);
    run(target, "run", generated);
}

fn run(target: &Utf8Path, command: &str, generated: &Utf8Path) {
    let mut cmd = std::process::Command::new("npm");
    cmd.arg(command);
    if command == "run" {
        cmd.arg("build");
    }
    let output = cmd.current_dir(target).output().unwrap();
    assert!(
        output.status.success(),
        "npm {command} failed for {generated}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
