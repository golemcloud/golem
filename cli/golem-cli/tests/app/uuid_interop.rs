// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

//! UUID values crossing every bridge generator in both bridge modes.
//!
//! `RustUuidTarget` is the hub: every SDK's guest bridge and every generator's
//! external bridge calls the same `RustUuidTarget(KEY)` instance, and the
//! Rust guest bridge calls the target agent of every other SDK. Each call
//! round-trips the key, the nil and the all-ones UUID at the top level, in an
//! option, in a list and in a record field, reads back the constructor key,
//! and increments a counter. The counter proves that every bridge computes the
//! same agent ID for a UUID constructor parameter.

use super::agents::{assert_generated_driver, run_generated_driver};
use super::{TestContext, cmd, flag};
use crate::workspace_path;
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use std::io::Write;
use test_r::{test, timeout};

const APP: &str = "uuid-interop";
const KEY: &str = "f0e1d2c3-b4a5-4697-8879-6a5b4c3d2e1f";

fn manifest() -> String {
    formatdoc! {r#"
        manifestVersion: {version}
        app: {APP}
        environments:
          local:
            server: local
            componentPresets: debug
        components:
          {APP}:rust-target:
            dir: rust-target
            templates: rust
          {APP}:ts:
            dir: ts
            templates: ts
            dependencies:
              agents:
                - {APP}:rust-target/RustUuidTarget
          {APP}:go:
            dir: go
            templates: go
            dependencies:
              agents:
                - {APP}:rust-target/RustUuidTarget
          {APP}:scala:
            dir: scala
            templates: scala
            dependencies:
              agents:
                - {APP}:rust-target/RustUuidTarget
          {APP}:moonbit:
            dir: moonbit
            templates: moonbit
            dependencies:
              agents:
                - {APP}:rust-target/RustUuidTarget
          {APP}:rust-caller:
            dir: rust-caller
            templates: rust
            dependencies:
              agents:
                - {APP}:ts/TsUuidTarget
                - {APP}:go/GoUuidTarget
                - {APP}:scala/ScalaUuidTarget
                - {APP}:moonbit/MoonbitUuidTarget
    "#, version = versions::sdk::MANIFEST}
}

fn write_rust_target(ctx: &TestContext) {
    fs::write_str(
        ctx.cwd_path_join("rust-target/src/lib.rs"),
        indoc! {r#"
            use golem_rust::{agent_definition, agent_implementation, FromSchema, FromWire, IntoSchema, IntoWire, Uuid, WireSchema};

            #[derive(Clone, Debug, PartialEq, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
            pub struct UuidHolder {
                pub primary: Uuid,
                pub maybe: Option<Uuid>,
                pub all: Vec<Uuid>,
            }

            #[agent_definition]
            pub trait RustUuidTarget {
                fn new(key: Uuid) -> Self;
                fn echo(&self, value: Uuid) -> Uuid;
                fn nested(&self, value: UuidHolder) -> UuidHolder;
                fn identity(&self) -> Uuid;
                fn touch(&mut self) -> u32;
            }

            struct RustUuidTargetImpl {
                key: Uuid,
                touches: u32,
            }

            #[agent_implementation]
            impl RustUuidTarget for RustUuidTargetImpl {
                fn new(key: Uuid) -> Self {
                    Self { key, touches: 0 }
                }
                fn echo(&self, value: Uuid) -> Uuid {
                    value
                }
                fn nested(&self, value: UuidHolder) -> UuidHolder {
                    value
                }
                fn identity(&self) -> Uuid {
                    self.key
                }
                fn touch(&mut self) -> u32 {
                    self.touches += 1;
                    self.touches
                }
            }
        "#},
    )
    .unwrap();
}

fn write_rust_caller(ctx: &TestContext) {
    fs::write_str(
        ctx.cwd_path_join("rust-caller/src/lib.rs"),
        indoc! {r#"
            use golem_rust::{agent_definition, agent_implementation, Uuid};

            macro_rules! exercise {
                ($client:ident, $agent:ident, $key:expr) => {{
                    use $client::{$agent, UuidHolder};
                    let key = $key;
                    let nil = Uuid::nil();
                    let max = Uuid::from_u128(u128::MAX);
                    let target = $agent::get(key).unwrap();
                    for value in [key, nil, max] {
                        assert_eq!(target.echo(value).await.unwrap(), value);
                    }
                    for maybe in [Some(max), None] {
                        let holder = target
                            .nested(UuidHolder { primary: key, maybe, all: vec![key, nil, max] })
                            .await
                            .unwrap();
                        assert_eq!(
                            (holder.primary, holder.maybe, holder.all),
                            (key, maybe, vec![key, nil, max])
                        );
                    }
                    assert_eq!(target.identity().await.unwrap(), key);
                    format!("ok:{}", target.touch().await.unwrap())
                }};
            }

            #[agent_definition]
            pub trait RustUuidCaller {
                fn new(name: String) -> Self;
                async fn run(&self, target: String, key: String) -> String;
            }

            struct RustUuidCallerImpl;

            #[agent_implementation]
            impl RustUuidCaller for RustUuidCallerImpl {
                fn new(_name: String) -> Self {
                    Self
                }
                async fn run(&self, target: String, key: String) -> String {
                    let key = Uuid::parse_str(&key).unwrap();
                    match target.as_str() {
                        "ts" => exercise!(ts_uuid_target_guest_client, TsUuidTarget, key),
                        "go" => exercise!(go_uuid_target_guest_client, GoUuidTarget, key),
                        "scala" => exercise!(scala_uuid_target_guest_client, ScalaUuidTarget, key),
                        "moonbit" => exercise!(moonbit_uuid_target_guest_client, MoonbitUuidTarget, key),
                        other => panic!("unknown target {other}"),
                    }
                }
            }
        "#},
    )
    .unwrap();
    let manifest = ctx.cwd_path_join("rust-caller/Cargo.toml");
    let cargo = fs::read_to_string(&manifest).unwrap();
    fs::write_str(
        &manifest,
        cargo.replace(
            "[dependencies]",
            indoc! {r#"
                [dependencies]
                ts-uuid-target-guest-client = { path = "../golem-temp/bridge-sdk/rust/internal/ts-uuid-target-guest-client" }
                go-uuid-target-guest-client = { path = "../golem-temp/bridge-sdk/rust/internal/go-uuid-target-guest-client" }
                scala-uuid-target-guest-client = { path = "../golem-temp/bridge-sdk/rust/internal/scala-uuid-target-guest-client" }
                moonbit-uuid-target-guest-client = { path = "../golem-temp/bridge-sdk/rust/internal/moonbit-uuid-target-guest-client" }
            "#}
            .trim_end(),
        ),
    )
    .unwrap();
}

fn write_ts(ctx: &TestContext) {
    let tsconfig_path = ctx.cwd_path_join("ts/tsconfig.json");
    let mut tsconfig: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&tsconfig_path).unwrap()).unwrap();
    tsconfig["compilerOptions"]["paths"]["rust-uuid-target-guest-client"] = serde_json::json!([
        "../golem-temp/bridge-sdk/ts/internal/rust-uuid-target-guest-client/rust-uuid-target-guest-client.ts"
    ]);
    tsconfig["include"].as_array_mut().unwrap().extend([
        serde_json::json!("src/**/*.ts"),
        serde_json::json!(
            "../golem-temp/bridge-sdk/ts/internal/rust-uuid-target-guest-client/*.ts"
        ),
    ]);
    fs::write_str(
        tsconfig_path,
        serde_json::to_string_pretty(&tsconfig).unwrap(),
    )
    .unwrap();
    fs::write_str(
        ctx.cwd_path_join("ts/src/counter-agent.ts"),
        indoc! {r#"
            import { z } from 'zod';
            import { defineAgent, method, s, Uuid } from '@golemcloud/golem-ts-sdk';
            import { RustUuidTarget } from 'rust-uuid-target-guest-client';

            const holder = z.object({ primary: s.uuid(), maybe: z.optional(s.uuid()), all: z.array(s.uuid()) });

            export const TsUuidTarget = defineAgent({
              name: 'TsUuidTarget', id: { key: s.uuid() },
              methods: {
                echo: method({ input: { value: s.uuid() }, returns: s.uuid() }),
                nested: method({ input: { value: holder }, returns: holder }),
                identity: method({ input: {}, returns: s.uuid() }),
                touch: method({ input: {}, returns: s.u32() }),
              },
            });

            export const TsUuidTargetImpl = TsUuidTarget.implement({
              init: ({ id }) => ({ key: id.key, touches: 0 }),
              methods: {
                echo({ value }) { return value; },
                nested({ value }) { return value; },
                identity() { return this.key; },
                touch() { this.touches += 1; return this.touches; },
              },
            });

            export const TsUuidCaller = defineAgent({
              name: 'TsUuidCaller', id: { name: z.string() },
              methods: { run: method({ input: { key: z.string() }, returns: z.string() }) },
            });

            const text = (value: Uuid | undefined) => value?.toString();
            const check = (ok: boolean, message: string) => { if (!ok) throw new Error(message); };

            export const TsUuidCallerImpl = TsUuidCaller.implement({
              init: () => ({}),
              methods: {
                async run({ key }) {
                  const id = Uuid.parse(key);
                  const nil = Uuid.parse('00000000-0000-0000-0000-000000000000');
                  const max = Uuid.parse('ffffffff-ffff-ffff-ffff-ffffffffffff');
                  const target = RustUuidTarget.get(id);
                  for (const value of [id, nil, max]) {
                    check(text(await target.echo(value)) === text(value), `echo ${text(value)}`);
                  }
                  for (const maybe of [max, undefined]) {
                    const back = await target.nested({ primary: id, maybe, all: [id, nil, max] });
                    check(text(back.primary) === key, 'nested primary');
                    check(text(back.maybe) === text(maybe), 'nested maybe');
                    check(back.all.map(text).join() === [id, nil, max].map(text).join(), 'nested all');
                  }
                  check(text(await target.identity()) === key, 'identity');
                  return `ok:${await target.touch()}`;
                },
              },
            });
        "#},
    )
    .unwrap();
}

fn write_go(ctx: &TestContext) {
    let dir = ctx.cwd_path_join("go");
    let go_mod = dir.join("go.mod");
    let module = fs::read_to_string(&go_mod)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("module ").map(|m| m.trim().to_string()))
        .expect("the Go component's go.mod names its module");
    fs::remove(dir.join("agents/counter")).unwrap();
    fs::write_str(
        dir.join("agents/uuids/uuids.go"),
        indoc! {r#"
            package uuids

            import (
            	"github.com/golemcloud/golem/sdks/go/core/values"
            	"github.com/golemcloud/golem/sdks/go/golem"
            )

            type UuidHolder struct {
            	Primary values.UUID
            	Maybe   values.Option[values.UUID]
            	All     []values.UUID
            }

            type TargetID struct{ Key values.UUID }

            type EchoIn struct{ Value values.UUID }

            type NestedIn struct{ Value UuidHolder }

            var Target = golem.DefineAgent[TargetID](golem.Spec{Name: "GoUuidTarget"})

            var (
            	Echo     = Target.Method[EchoIn, values.UUID]("echo")
            	Nested   = Target.Method[NestedIn, UuidHolder]("nested")
            	Identity = Target.Method[golem.Unit, values.UUID]("identity")
            	Touch    = Target.Method[golem.Unit, uint32]("touch")
            )

            type CallerID struct{ Name string }

            type RunIn struct{ Key string }

            var Caller = golem.DefineAgent[CallerID](golem.Spec{Name: "GoUuidCaller"})

            var Run = Caller.Method[RunIn, string]("run")
        "#},
    )
    .unwrap();
    fs::write_str(
        dir.join("agents/uuids/impl/impl.go"),
        formatdoc! {r#"
            package impl

            import (
            	"fmt"
            	"slices"

            	"{module}/agents/uuids"

            	rust "golem.local/bridge/rust-uuid-target-guest-client"

            	"github.com/golemcloud/golem/sdks/go/core/values"
            	"github.com/golemcloud/golem/sdks/go/golem"
            )

            type targetState struct {{
            	key     values.UUID
            	touches uint32
            }}

            type callerState struct{{}}

            var target = uuids.Target.Implement(func(id uuids.TargetID) *targetState {{ return &targetState{{key: id.Key}} }})

            var caller = uuids.Caller.Implement(func(uuids.CallerID) *callerState {{ return &callerState{{}} }})

            func check(ok bool, format string, args ...any) {{
            	if !ok {{
            		panic(fmt.Sprintf(format, args...))
            	}}
            }}

            func init() {{
            	target.Handle(uuids.Echo, func(_ *golem.Context[targetState], in uuids.EchoIn) values.UUID {{ return in.Value }})
            	target.Handle(uuids.Nested, func(_ *golem.Context[targetState], in uuids.NestedIn) uuids.UuidHolder {{ return in.Value }})
            	target.Handle(uuids.Identity, func(ctx *golem.Context[targetState], _ golem.Unit) values.UUID {{ return ctx.State.key }})
            	target.Handle(uuids.Touch, func(ctx *golem.Context[targetState], _ golem.Unit) uint32 {{
            		ctx.State.touches++
            		return ctx.State.touches
            	}})
            	caller.Handle(uuids.Run, func(_ *golem.Context[callerState], in uuids.RunIn) string {{
            		key := golem.Must(values.ParseUUID(in.Key))
            		var nilUUID values.UUID
            		maxUUID := golem.Must(values.ParseUUID("ffffffff-ffff-ffff-ffff-ffffffffffff"))
            		remote := golem.Must(rust.GetRustUUIDTarget(rust.RustUUIDTargetID{{Key: key}}))
            		for _, value := range []values.UUID{{key, nilUUID, maxUUID}} {{
            			echoed := golem.Must(remote.Echo(value))
            			check(echoed == value, "echo %s returned %s", value, echoed)
            		}}
            		for _, maybe := range []values.Option[values.UUID]{{values.Some(maxUUID), values.None[values.UUID]()}} {{
            			all := []values.UUID{{key, nilUUID, maxUUID}}
            			back := golem.Must(remote.Nested(rust.UUIDHolder{{Primary: key, Maybe: maybe, All: all}}))
            			check(back.Primary == key, "nested primary %s", back.Primary)
            			check(back.Maybe == maybe, "nested maybe")
            			check(slices.Equal(back.All, all), "nested all %v", back.All)
            		}}
            		check(golem.Must(remote.Identity()) == key, "identity")
            		return fmt.Sprintf("ok:%d", golem.Must(remote.Touch()))
            	}})
            }}
        "#},
    )
    .unwrap();
    let main_go = dir.join("main.go");
    fs::write_str(
        &main_go,
        fs::read_to_string(&main_go).unwrap().replace(
            &format!("{module}/agents/counter/impl"),
            &format!("{module}/agents/uuids/impl"),
        ),
    )
    .unwrap();
    fs::write_str(
        &go_mod,
        fs::read_to_string(&go_mod).unwrap()
            + indoc! {r#"

                require golem.local/bridge/rust-uuid-target-guest-client v0.0.0

                replace golem.local/bridge/rust-uuid-target-guest-client => ../golem-temp/bridge-sdk/go/internal/rust-uuid-target-guest-client
            "#},
    )
    .unwrap();
}

fn write_scala(ctx: &TestContext) {
    let scala_dir = ctx.cwd_path_join("scala/src/main/scala");
    std::fs::remove_dir_all(&scala_dir).unwrap();
    fs::create_dir_all(scala_dir.join("uuids")).unwrap();
    fs::write_str(
        scala_dir.join("uuids/UuidAgents.scala"),
        indoc! {r#"
            package uuids

            import golem.{BaseAgent, Uuid}
            import golem.runtime.annotations.{agentDefinition, agentImplementation}
            import golem.bridge.client.rust_uuid_target.{RustUuidTargetClient, UuidHolder => RustHolder}
            import zio.blocks.schema.Schema
            import scala.concurrent.Future
            import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

            final case class UuidHolder(primary: Uuid, maybe: Option[Uuid], all: List[Uuid])
            object UuidHolder {
              implicit val schema: Schema[UuidHolder] = Schema.derived
            }

            @agentDefinition()
            trait ScalaUuidTarget extends BaseAgent {
              class Id(val key: Uuid)
              def echo(value: Uuid): Uuid
              def nested(value: UuidHolder): UuidHolder
              def identity(): Uuid
              def touch(): Int
            }

            @agentImplementation()
            final class ScalaUuidTargetImpl(private val key: Uuid) extends ScalaUuidTarget {
              private var touches = 0
              def echo(value: Uuid): Uuid = value
              def nested(value: UuidHolder): UuidHolder = value
              def identity(): Uuid = key
              def touch(): Int = { touches += 1; touches }
            }

            @agentDefinition()
            trait ScalaUuidCaller extends BaseAgent {
              class Id(val name: String)
              def run(key: String): Future[String]
            }

            @agentImplementation()
            final class ScalaUuidCallerImpl(private val name: String) extends ScalaUuidCaller {
              private def parse(text: String): Uuid = Uuid.fromStandardString(text).toOption.get

              def run(key: String): Future[String] = {
                val id = parse(key)
                val nil = parse("00000000-0000-0000-0000-000000000000")
                val max = parse("ffffffff-ffff-ffff-ffff-ffffffffffff")
                val target = RustUuidTargetClient.get(id)
                for {
                  echoed <- Future.sequence(List(id, nil, max).map(target.echo))
                  withSome <- target.nested(RustHolder(id, Some(max), List(id, nil, max)))
                  withNone <- target.nested(RustHolder(id, None, List(id, nil, max)))
                  identity <- target.identity()
                  touches <- target.touch()
                } yield {
                  require(echoed == List(id, nil, max), s"echo $echoed")
                  require(withSome == RustHolder(id, Some(max), List(id, nil, max)), s"nested $withSome")
                  require(withNone == RustHolder(id, None, List(id, nil, max)), s"nested $withNone")
                  require(identity == id, s"identity $identity")
                  s"ok:${touches.value}"
                }
              }
            }
        "#},
    )
    .unwrap();
    let build_path = ctx.cwd_path_join("uuid_interop_scala.sbt");
    let mut build = fs::read_to_string(&build_path).unwrap();
    build.push_str("\n  .settings(Compile / unmanagedSourceDirectories += (LocalRootProject / baseDirectory).value / \"golem-temp/bridge-sdk/scala/internal/rust-uuid-target-guest-client/src/main/scala\")\n");
    fs::write_str(build_path, build).unwrap();
}

fn write_moonbit(ctx: &TestContext) {
    let manifest = ctx.cwd_path_join("moon.mod.json");
    let mut module: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
    module["deps"]["rust-uuid-target-guest-client"] = serde_json::json!({"path": "golem-temp/bridge-sdk/moonbit/internal/rust-uuid-target-guest-client"});
    fs::write_str(
        &manifest,
        serde_json::to_string_pretty(&module).unwrap() + "\n",
    )
    .unwrap();
    let manifest = ctx.cwd_path_join("moonbit/moon.pkg");
    let package = fs::read_to_string(&manifest).unwrap().replace(
        "import {",
        "import {\n  \"rust-uuid-target-guest-client/client\" @rust,",
    );
    fs::write_str(&manifest, package).unwrap();
    fs::write_str(
        ctx.cwd_path_join("moonbit/counter.mbt"),
        indoc! {r#"
            ///|
            #derive.golem_schema
            pub(all) struct UuidHolder {
              primary : @schema.Uuid
              maybe : @schema.Uuid?
              all : Array[@schema.Uuid]
            }

            ///|
            #derive.agent
            struct MoonbitUuidTarget {
              key : @schema.Uuid
              mut touches : UInt
            }

            ///|
            fn MoonbitUuidTarget::new(key : @schema.Uuid) -> MoonbitUuidTarget {
              { key, touches: 0 }
            }

            ///|
            pub fn MoonbitUuidTarget::echo(_self : Self, value : @schema.Uuid) -> @schema.Uuid {
              value
            }

            ///|
            pub fn MoonbitUuidTarget::nested(_self : Self, value : UuidHolder) -> UuidHolder {
              value
            }

            ///|
            pub fn MoonbitUuidTarget::identity(self : Self) -> @schema.Uuid {
              self.key
            }

            ///|
            pub fn MoonbitUuidTarget::touch(self : Self) -> UInt {
              self.touches += 1
              self.touches
            }

            ///|
            #derive.agent
            struct MoonbitUuidCaller {
              name : String
            }

            ///|
            fn MoonbitUuidCaller::new(name : String) -> MoonbitUuidCaller {
              { name, }
            }

            ///|
            pub async fn MoonbitUuidCaller::run(_self : Self, key : String) -> String {
              let id = @schema.Uuid::parse(key) catch { _ => abort("invalid key") }
              let nil = @schema.Uuid::from_bits(0, 0)
              let max = @schema.Uuid::from_bits(0xFFFFFFFFFFFFFFFFUL, 0xFFFFFFFFFFFFFFFFUL)
              @rust.RustUuidTargetClient::scoped(id, async fn(remote) {
                for value in [id, nil, max] {
                  assert_eq(remote.echo(value), value)
                }
                for maybe in [Some(max), None] {
                  let back = remote.nested({ primary: id, maybe, all: [id, nil, max] })
                  assert_eq(back.primary, id)
                  assert_eq(back.maybe, maybe)
                  assert_eq(back.all, [id, nil, max])
                }
                assert_eq(remote.identity(), id)
                "ok:\{remote.touch()}"
              })
            }

            ///|
            fn main {}
        "#},
    )
    .unwrap();
}

async fn deployed_context() -> TestContext {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    fs::create_dir_all(ctx.cwd_path_join(APP)).unwrap();
    ctx.cd(APP);
    for (template, component) in [
        ("rust", "rust-target"),
        ("ts", "ts"),
        ("go", "go"),
        ("scala", "scala"),
        ("moonbit", "moonbit"),
        ("rust", "rust-caller"),
    ] {
        let output = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                template,
                flag::COMPONENT_NAME,
                &format!("{APP}:{component}"),
            ])
            .await;
        assert!(output.success_or_dump());
    }
    fs::write_str(ctx.cwd_path_join("golem.yaml"), manifest()).unwrap();
    write_rust_target(&ctx);
    write_ts(&ctx);
    write_go(&ctx);
    write_scala(&ctx);
    write_moonbit(&ctx);
    write_rust_caller(&ctx);
    assert!(ctx.cli([flag::YES, cmd::BUILD]).await.success_or_dump());
    assert!(ctx.cli([flag::YES, cmd::DEPLOY]).await.success_or_dump());
    ctx
}

async fn invoke(ctx: &TestContext, agent: &str, args: &[&str]) -> String {
    let mut command = vec![flag::YES, cmd::AGENT, cmd::INVOKE, agent];
    command.extend_from_slice(args);
    let output = ctx.cli(command).await;
    assert!(output.success_or_dump());
    output.stdout().collect::<Vec<_>>().join("\n")
}

async fn run_external_drivers(ctx: &TestContext, first_touch: u32) {
    let url = ctx.worker_service_url();
    let token = golem_client::LOCAL_WELL_KNOWN_TOKEN;
    let mut touch = first_touch;

    for language in ["rust", "typescript", "scala", "go", "moonbit"] {
        let root = ctx.cwd_path_join(format!("{language}-uuid-bridge"));
        let output = ctx
            .cli([
                cmd::GENERATE_BRIDGE,
                flag::LANGUAGE,
                language,
                flag::AGENT_TYPE_NAME,
                "RustUuidTarget",
                flag::OUTPUT_DIR,
                root.to_str().unwrap(),
            ])
            .await;
        assert!(output.success_or_dump());
    }

    let rust_client = ctx.cwd_path_join("rust-uuid-bridge/rust-uuid-target-client");
    let mut rust_manifest = std::fs::OpenOptions::new()
        .append(true)
        .open(rust_client.join("Cargo.toml"))
        .unwrap();
    writeln!(
        rust_manifest,
        "\n[dev-dependencies]\ntokio = {{ version = \"1\", features = [\"macros\", \"rt-multi-thread\"] }}"
    )
    .unwrap();
    std::fs::create_dir_all(rust_client.join("examples")).unwrap();
    std::fs::write(
        rust_client.join("examples/e2e.rs"),
        formatdoc! {r#"
            use rust_uuid_target_client::{{configure, GolemServer, RustUuidTarget, UuidHolder}};
            use uuid::Uuid;

            #[tokio::main]
            async fn main() -> Result<(), Box<dyn std::error::Error>> {{
                configure(
                    GolemServer::Custom {{ url: "{url}".parse()?, token: "{token}".to_string() }},
                    "{APP}",
                    "local",
                );
                let key = Uuid::parse_str("{KEY}")?;
                let nil = Uuid::nil();
                let max = Uuid::from_u128(u128::MAX);
                let target = RustUuidTarget::get(key).await?;
                for value in [key, nil, max] {{
                    assert_eq!(target.echo(value).await?, value);
                }}
                for maybe in [Some(max), None] {{
                    let holder = target.nested(UuidHolder {{ primary: key, maybe, all: vec![key, nil, max] }}).await?;
                    assert_eq!((holder.primary, holder.maybe, holder.all), (key, maybe, vec![key, nil, max]));
                }}
                assert_eq!(target.identity().await?, key);
                println!("UUID_E2E_TOUCH {{}}", target.touch().await?);
                Ok(())
            }}
        "#},
    )
    .unwrap();
    let mut rust_command = std::process::Command::new("cargo");
    rust_command
        .args(["run", "--quiet", "--example", "e2e"])
        .env(
            "CARGO_TARGET_DIR",
            ctx.cwd_path_join("rust-uuid-bridge-target"),
        )
        .current_dir(&rust_client);
    assert_generated_driver(
        run_generated_driver(rust_command).await,
        "Rust",
        &format!("UUID_E2E_TOUCH {touch}"),
    );
    touch += 1;

    let typescript_client = ctx.cwd_path_join("typescript-uuid-bridge/rust-uuid-target-client");
    std::fs::write(
        typescript_client.join("e2e.ts"),
        formatdoc! {r#"
            import assert from 'node:assert/strict'
            import {{ Uuid }} from '@golemcloud/golem-ts-bridge'
            import {{ configure, RustUuidTarget }} from './rust-uuid-target-client.js'

            configure({{
              server: {{ type: 'custom', url: '{url}', token: '{token}' }},
              application: '{APP}',
              environment: 'local',
            }})
            const text = (value?: Uuid) => value?.toString()
            const key = Uuid.parse('{KEY}')
            const nil = Uuid.parse('00000000-0000-0000-0000-000000000000')
            const max = Uuid.parse('ffffffff-ffff-ffff-ffff-ffffffffffff')
            const target = await RustUuidTarget.get(key)
            for (const value of [key, nil, max]) assert.equal(text(await target.echo(value)), text(value))
            for (const maybe of [max, undefined]) {{
              const holder = await target.nested({{ primary: key, maybe, all: [key, nil, max] }})
              assert.equal(text(holder.primary), text(key))
              assert.equal(text(holder.maybe), text(maybe))
              assert.deepEqual(holder.all.map(text), [key, nil, max].map(text))
            }}
            assert.equal(text(await target.identity()), text(key))
            console.log(`UUID_E2E_TOUCH ${{await target.touch()}}`)
        "#},
    )
    .unwrap();
    let mut typescript_install = std::process::Command::new("pnpm");
    typescript_install
        .args(["install", "--ignore-workspace", "--ignore-scripts"])
        .current_dir(&typescript_client);
    assert_generated_driver(
        run_generated_driver(typescript_install).await,
        "TypeScript install",
        "",
    );
    let mut typescript_command = std::process::Command::new("pnpm");
    typescript_command
        .args(["exec", "tsx", "e2e.ts"])
        .current_dir(&typescript_client);
    assert_generated_driver(
        run_generated_driver(typescript_command).await,
        "TypeScript",
        &format!("UUID_E2E_TOUCH {touch}"),
    );
    touch += 1;

    let scala_client = ctx.cwd_path_join("scala-uuid-bridge/rust-uuid-target-client");
    std::fs::write(
        scala_client.join("src/main/scala/Main.scala"),
        formatdoc! {r#"
            import golem.bridge.client.rust_uuid_target.{{RustUuidTargetClient, UuidHolder}}
            import golem.bridge.runtime.{{GolemServer, Uuid}}
            import scala.concurrent.{{Await, ExecutionContext}}
            import scala.concurrent.duration.*

            object Main {{
              given ExecutionContext = ExecutionContext.global
              val timeout = 90.seconds

              def main(args: Array[String]): Unit = {{
                RustUuidTargetClient.configure(GolemServer.Custom("{url}", "{token}"), "{APP}", "local")
                def parse(text: String): Uuid = Uuid.fromStandardString(text).toOption.get
                val key = parse("{KEY}")
                val nil = parse("00000000-0000-0000-0000-000000000000")
                val max = parse("ffffffff-ffff-ffff-ffff-ffffffffffff")
                val target = Await.result(RustUuidTargetClient.get(key), timeout)
                for (value <- List(key, nil, max)) assert(Await.result(target.echo(value), timeout) == value)
                for (maybe <- List(Some(max), None)) {{
                  val holder = UuidHolder(key, maybe, List(key, nil, max))
                  assert(Await.result(target.nested(holder), timeout) == holder)
                }}
                assert(Await.result(target.identity(), timeout) == key)
                println(s"UUID_E2E_TOUCH ${{Await.result(target.touch(), timeout).value}}")
              }}
            }}
        "#},
    )
    .unwrap();
    let mut scala_command = std::process::Command::new("sbt");
    scala_command
        .args(["--batch", "runMain Main"])
        .current_dir(&scala_client);
    assert_generated_driver(
        run_generated_driver(scala_command).await,
        "Scala",
        &format!("UUID_E2E_TOUCH {touch}"),
    );
    touch += 1;

    let go_program = ctx.cwd_path_join("go-uuid-e2e");
    std::fs::create_dir_all(&go_program).unwrap();
    let sdks = workspace_path().join("sdks/go");
    std::fs::write(
        go_program.join("go.mod"),
        formatdoc! {r#"
            module example.com/go-uuid-e2e

            go {go}

            require (
            	golem.local/bridge/rust-uuid-target-client v0.0.0
            	github.com/golemcloud/golem/sdks/go/bridge v0.0.0
            	github.com/golemcloud/golem/sdks/go/core v0.0.0
            )

            replace golem.local/bridge/rust-uuid-target-client => {client}

            replace github.com/golemcloud/golem/sdks/go/bridge => {bridge}

            replace github.com/golemcloud/golem/sdks/go/core => {core}
            "#,
            go = versions::build_tool::GO_MIN,
            client = ctx.cwd_path_join("go-uuid-bridge/rust-uuid-target-client").display(),
            bridge = sdks.join("bridge").display(),
            core = sdks.join("core").display(),
        },
    )
    .unwrap();
    std::fs::write(
        go_program.join("main.go"),
        formatdoc! {r#"
            package main

            import (
            	"context"
            	"fmt"
            	"log"
            	"slices"

            	"github.com/golemcloud/golem/sdks/go/bridge"
            	"github.com/golemcloud/golem/sdks/go/core/values"
            	client "golem.local/bridge/rust-uuid-target-client"
            )

            func must[T any](value T, err error) T {{
            	if err != nil {{
            		log.Fatal(err)
            	}}
            	return value
            }}

            func main() {{
            	if err := bridge.Configure(bridge.Configuration{{
            		Server:  bridge.Custom("{url}", "{token}"),
            		AppName: "{APP}",
            		EnvName: "local",
            	}}); err != nil {{
            		log.Fatal(err)
            	}}
            	ctx := context.Background()
            	key := must(values.ParseUUID("{KEY}"))
            	var nilUUID values.UUID
            	maxUUID := must(values.ParseUUID("ffffffff-ffff-ffff-ffff-ffffffffffff"))
            	target := must(client.GetRustUUIDTarget(client.RustUUIDTargetID{{Key: key}}))
            	for _, value := range []values.UUID{{key, nilUUID, maxUUID}} {{
            		if echoed := must(target.Echo(ctx, value)); echoed != value {{
            			log.Fatalf("echo %s returned %s", value, echoed)
            		}}
            	}}
            	for _, maybe := range []values.Option[values.UUID]{{values.Some(maxUUID), values.None[values.UUID]()}} {{
            		all := []values.UUID{{key, nilUUID, maxUUID}}
            		back := must(target.Nested(ctx, client.UUIDHolder{{Primary: key, Maybe: maybe, All: all}}))
            		if back.Primary != key || back.Maybe != maybe || !slices.Equal(back.All, all) {{
            			log.Fatalf("nested returned %+v", back)
            		}}
            	}}
            	if identity := must(target.Identity(ctx)); identity != key {{
            		log.Fatalf("identity returned %s", identity)
            	}}
            	fmt.Printf("UUID_E2E_TOUCH %d\n", must(target.Touch(ctx)))
            }}
        "#},
    )
    .unwrap();
    let mut go_command = std::process::Command::new("go");
    go_command.args(["run", "."]).current_dir(&go_program);
    assert_generated_driver(
        run_generated_driver(go_command).await,
        "Go",
        &format!("UUID_E2E_TOUCH {touch}"),
    );
    touch += 1;

    let moonbit_client = ctx.cwd_path_join("moonbit-uuid-bridge/rust-uuid-target-client");
    let moonbit_manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(moonbit_client.join("moon.mod.json")).unwrap(),
    )
    .unwrap();
    let moonbit_module = moonbit_manifest["name"].as_str().unwrap();
    let moonbit_main_dir = moonbit_client.join("main");
    std::fs::create_dir_all(&moonbit_main_dir).unwrap();
    std::fs::write(
        moonbit_main_dir.join("moon.pkg"),
        formatdoc! {r#"
            import {{
              "moonbitlang/async",
              "{moonbit_module}/client" @client,
              "{moonbit_module}/runtime" @runtime,
            }}

            options(
              "is-main": true,
            )
        "#},
    )
    .unwrap();
    std::fs::write(
        moonbit_main_dir.join("main.mbt"),
        formatdoc! {r#"
            async fn main {{
              @client.RustUuidTarget::configure(@runtime.Custom("{url}", "{token}"), "{APP}", "local")
              let key = @runtime.Uuid::parse("{KEY}")
              let nil = @runtime.Uuid::from_bits(0, 0)
              let max = @runtime.Uuid::from_bits(0xFFFFFFFFFFFFFFFFUL, 0xFFFFFFFFFFFFFFFFUL)
              let target = @client.RustUuidTarget::get(key)
              for value in [key, nil, max] {{
                assert_eq(target.echo(value), value)
              }}
              for maybe in [Some(max), None] {{
                let holder = target.nested({{ primary: key, maybe, all: [key, nil, max] }})
                assert_eq(holder.primary, key)
                assert_eq(holder.maybe, maybe)
                assert_eq(holder.all, [key, nil, max])
              }}
              assert_eq(target.identity(), key)
              println("UUID_E2E_TOUCH \{{target.touch()}}")
            }}
        "#},
    )
    .unwrap();
    let mut moonbit_command = std::process::Command::new("moon");
    moonbit_command
        .args(["run", "--target", "native", "--deny-warn", "main"])
        .current_dir(&moonbit_client);
    assert_generated_driver(
        run_generated_driver(moonbit_command).await,
        "MoonBit",
        &format!("UUID_E2E_TOUCH {touch}"),
    );
}

#[test]
#[timeout("60 minutes")]
async fn uuid_values_cross_every_bridge() {
    let ctx = deployed_context().await;

    let mut touch = 1;
    for (caller, label) in [
        ("TsUuidCaller(\"hub\")", "ts"),
        ("GoUuidCaller(\"hub\")", "go"),
        ("ScalaUuidCaller(\"hub\")", "scala"),
        ("MoonbitUuidCaller(\"hub\")", "moonbit"),
    ] {
        let stdout = invoke(&ctx, caller, &["run", &format!("\"{KEY}\"")]).await;
        assert!(
            stdout.contains(&format!("ok:{touch}")),
            "{label} guest caller of RustUuidTarget: {stdout}"
        );
        touch += 1;
    }

    for target in ["ts", "go", "scala", "moonbit"] {
        let stdout = invoke(
            &ctx,
            "RustUuidCaller(\"hub\")",
            &["run", &format!("\"{target}\""), &format!("\"{KEY}\"")],
        )
        .await;
        assert!(
            stdout.contains("ok:1"),
            "Rust guest caller of the {target} target: {stdout}"
        );
    }

    run_external_drivers(&ctx, touch).await;
}
