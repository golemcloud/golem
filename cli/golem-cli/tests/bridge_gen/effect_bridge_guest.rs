// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use crate::bridge_gen::fixtures::{
    agent, def, field, guest_streaming_agent_type, local_config, method, ref_to,
};
use camino::Utf8Path;
use golem_cli::bridge_gen::BridgeGenerator;
use golem_cli::bridge_gen::effect::effect_guest::EffectGuestBridgeGenerator;
use golem_common::model::agent::AgentMode;
use golem_common::schema::SchemaType;
use golem_common::schema::schema_type::{PermissionCardSpec, ResultSpec};
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
import { Effect, Stream } from "effect"
import { GuestStreamingAgent } from "./guest-streaming-agent-guest-client.js"
const client = GuestStreamingAgent.getWithConfig("id", 10n)
const call = Effect.flatMap(client, value => value.status())
const streamCall = Effect.flatMap(client, value => value.produce())
const checked: Effect.Effect<Stream.Stream<unknown, unknown>, unknown, unknown> = streamCall
void call
void checked
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

#[test]
fn effect_guest_stream_agent_and_schema_name_compile() {
    let schema = agent(
        "Stream",
        "effect",
        vec![],
        vec![method(
            "produce",
            vec![],
            Some(SchemaType::stream(Some(ref_to("EffectStream")))),
        )],
        vec![def("EffectStream", SchemaType::string())],
        AgentMode::Durable,
    );
    generate_and_compile(
        schema,
        "stream-guest-client.ts",
        r#"
import { Effect } from "effect"
import { Stream as GeneratedStream } from "./stream-guest-client.js"
const produced = GeneratedStream.get().pipe(Effect.flatMap(client => client.produce()))
void produced
"#,
    );
}

#[test]
fn effect_guest_scope_cards_on_awaited_calls_compile() {
    let card = || SchemaType::permission_card(PermissionCardSpec { polymorphic: false });
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let durable = mode == AgentMode::Durable;
        let schema = agent(
            "CardAgent",
            "effect",
            vec![field("name", SchemaType::string())],
            vec![
                method(
                    "give",
                    vec![field("card", card())],
                    Some(SchemaType::string()),
                ),
                method("take", vec![], Some(card())),
                method("withScopeCard", vec![], Some(SchemaType::string())),
                method("scopeCard", vec![], Some(SchemaType::string())),
            ],
            vec![],
            mode,
        );
        let (factory, value) = if durable {
            ("CardAgent.get(\"a\")", "string")
        } else {
            (
                "CardAgent.newPhantom(\"a\")",
                "{ readonly metadata: unknown; readonly value: string }",
            )
        };
        let source = generate_and_compile(
            schema,
            "card-agent-guest-client.ts",
            &format!(
                r#"
import {{ Effect }} from "effect"
import {{ Permissions }} from "@golemcloud/effect-golem"
import {{ CardAgent }} from "./card-agent-guest-client.js"
declare const card: Permissions.PermissionCard
declare const scope: Permissions.ScopeCard
const program = Effect.gen(function* () {{
  const client = yield* {factory}
  const scoped: CardAgent = client.withScopeCard(scope)
  const viaDefault: {value} = yield* scoped.give(card)
  const perCall: {value} = yield* client.give.withScopeCard(scope)(card)
  const cleared: {value} = yield* scoped.give.withScopeCard(undefined)(card)
  yield* client.take.withScopeCard(scope)()
  yield* client.give.trigger(card)
  const renamedMethod: {value} = yield* client.withScopeCard1()
  const renamedField: {value} = yield* client.scopeCard1.withScopeCard(scope)()
  // @ts-expect-error triggers never carry a scope card
  void client.give.trigger.withScopeCard
  // @ts-expect-error a persistent card is not a client default scope card
  void client.withScopeCard(card)
  // @ts-expect-error a persistent card is not a per-call scope card
  void client.give.withScopeCard(card)
  // @ts-expect-error a scope card is not a schema permission card
  void client.give(scope)
  return [viaDefault, perCall, cleared, renamedMethod, renamedField]
}})
void program
"#
            ),
        );
        assert!(
            source.contains("base.permissionCardToSchemaValue(")
                && source.contains("base.permissionCardFromSchemaValue("),
            "generated card codecs use the renamed bridge symbols"
        );
    }
}

#[test]
fn effect_guest_polymorphic_permission_cards_compile() {
    let concrete = || SchemaType::permission_card(PermissionCardSpec { polymorphic: false });
    let template = || SchemaType::permission_card(PermissionCardSpec { polymorphic: true });
    let schema = agent(
        "TemplateAgent",
        "effect",
        vec![field("name", SchemaType::string())],
        vec![
            method(
                "giveTemplate",
                vec![field("card", template())],
                Some(template()),
            ),
            method(
                "giveConcrete",
                vec![field("card", concrete())],
                Some(concrete()),
            ),
        ],
        vec![],
        AgentMode::Durable,
    );
    let source = generate_and_compile(
        schema,
        "template-agent-guest-client.ts",
        r#"
import { Effect } from "effect"
import { Permissions } from "@golemcloud/effect-golem"
import { TemplateAgent } from "./template-agent-guest-client.js"
declare const card: Permissions.PermissionCard
declare const template: Permissions.PolymorphicPermissionCard
const program = Effect.gen(function* () {
  const client = yield* TemplateAgent.get("a")
  const returnedTemplate: Permissions.PolymorphicPermissionCard = yield* client.giveTemplate(template)
  const returnedCard: Permissions.PermissionCard = yield* client.giveConcrete(card)
  // @ts-expect-error a concrete card is not a polymorphic card
  void client.giveTemplate(card)
  // @ts-expect-error a polymorphic card is not a concrete card
  void client.giveConcrete(template)
  // @ts-expect-error a polymorphic result is not a concrete card
  const wrong: Permissions.PermissionCard = yield* client.giveTemplate(template)
  return [returnedTemplate, returnedCard, wrong]
})
void program
"#,
    );
    for symbol in [
        "base.polymorphicPermissionCardToSchemaValue(",
        "base.polymorphicPermissionCardFromSchemaValue(",
        "base.permissionCardToSchemaValue(",
        "base.permissionCardFromSchemaValue(",
    ] {
        assert!(source.contains(symbol), "missing {symbol}:\n{source}");
    }
}

#[test]
fn effect_guest_effect_results_and_trailing_optional_parameters_compile() {
    let result = |ok: SchemaType, err: SchemaType| {
        SchemaType::result(ResultSpec {
            ok: Some(Box::new(ok)),
            err: Some(Box::new(err)),
        })
    };
    let schema = agent(
        "OptionalEffectAgent",
        "effect",
        vec![
            field("name", SchemaType::string()),
            field("nickname", SchemaType::option(SchemaType::string())),
        ],
        vec![
            method(
                "lookup",
                vec![
                    field("first", SchemaType::option(SchemaType::string())),
                    field("count", SchemaType::u32()),
                    field("last", SchemaType::option(SchemaType::string())),
                ],
                Some(result(SchemaType::u64(), SchemaType::string())),
            ),
            method(
                "accept",
                vec![field(
                    "outcome",
                    result(SchemaType::string(), SchemaType::u32()),
                )],
                Some(SchemaType::string()),
            ),
        ],
        vec![],
        AgentMode::Durable,
    );
    let source = generate_and_compile(
        schema,
        "optional-effect-agent-guest-client.ts",
        r#"
import { Effect, Result } from "effect"
import { OptionalEffectAgent } from "./optional-effect-agent-guest-client.js"
const program = Effect.gen(function*() {
  const client = yield* OptionalEffectAgent.get("name")
  yield* OptionalEffectAgent.get("name", "nick")
  const found: Result.Result<bigint, string> = yield* client.lookup(undefined, 1)
  const count: bigint = Result.isSuccess(found) ? found.success : 0n
  yield* client.lookup("first", 1, "last")
  // @ts-expect-error an optional parameter before a required one cannot be omitted
  yield* client.lookup(1)
  const accepted: string = yield* client.accept(Result.succeed("yes"))
  yield* client.accept(Result.fail(1))
  return [count, accepted] as const
})
void program
"#,
    );
    assert!(
        source.contains("[outcome: Result.Result<string, number>]"),
        "{source}"
    );
    assert!(
        source.contains("[first: string | undefined, count: number, last?: string]"),
        "{source}"
    );
    assert!(source.contains("Result.succeed("), "{source}");
    assert!(!source.contains("{ ok:"), "{source}");
}

fn generate_and_compile(
    schema: golem_common::schema::AgentTypeSchema,
    filename: &str,
    consumer: &str,
) -> String {
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
    source
}
