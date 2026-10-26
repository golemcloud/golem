// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

//! Quantity values crossing every bridge generator in both bridge modes.
//!
//! Every target agent declares one length unit: base `m`, allowed `m`, `cm`
//! and `km`. `RustQuantityTarget` is the hub: every SDK's guest bridge and
//! every generator's external bridge calls it, and the Rust guest bridge calls
//! the target agent of every other SDK. Each caller round-trips three
//! quantities at the top level, in an option, in a list and in a record field,
//! and checks the mantissa, the scale and the unit. Each caller also builds or
//! sends a quantity in `mi` and expects its bridge to reject it.

use super::agents::{assert_generated_driver, run_generated_driver};
use super::{TestContext, cmd, flag};
use crate::workspace_path;
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use std::io::Write;
use test_r::{test, timeout};

const APP: &str = "quantity-interop";

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
                - {APP}:rust-target/RustQuantityTarget
          {APP}:go:
            dir: go
            templates: go
            dependencies:
              agents:
                - {APP}:rust-target/RustQuantityTarget
          {APP}:scala:
            dir: scala
            templates: scala
            dependencies:
              agents:
                - {APP}:rust-target/RustQuantityTarget
          {APP}:moonbit:
            dir: moonbit
            templates: moonbit
            dependencies:
              agents:
                - {APP}:rust-target/RustQuantityTarget
          {APP}:scala-caller:
            dir: scala-caller
            templates: scala
            dependencies:
              agents:
                - {APP}:scala/ScalaQuantityTarget
          {APP}:rust-caller:
            dir: rust-caller
            templates: rust
            dependencies:
              agents:
                - {APP}:ts/TsQuantityTarget
                - {APP}:go/GoQuantityTarget
                - {APP}:scala/ScalaQuantityTarget
                - {APP}:moonbit/MoonbitQuantityTarget
    "#, version = versions::sdk::MANIFEST}
}

fn write_rust_target(ctx: &TestContext) {
    fs::write_str(
        ctx.cwd_path_join("rust-target/src/lib.rs"),
        indoc! {r#"
            use golem_rust::{agent_definition, agent_implementation, FromSchema, FromWire, IntoSchema, IntoWire, Quantity, QuantityUnit, WireSchema};

            pub struct Length;

            impl QuantityUnit for Length {
                fn type_id() -> golem_rust::schema::TypeId {
                    golem_rust::schema::TypeId::new("quantity_interop.Length")
                }
                fn base_unit() -> &'static str {
                    "m"
                }
                fn allowed_suffixes() -> &'static [&'static str] {
                    &["m", "cm", "km"]
                }
            }

            #[derive(Clone, Debug, PartialEq, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
            pub struct QuantityHolder {
                pub primary: Quantity<Length>,
                pub maybe: Option<Quantity<Length>>,
                pub all: Vec<Quantity<Length>>,
            }

            #[agent_definition]
            pub trait RustQuantityTarget {
                fn new(name: String) -> Self;
                fn echo(&self, value: Quantity<Length>) -> Quantity<Length>;
                fn nested(&self, value: QuantityHolder) -> QuantityHolder;
            }

            struct RustQuantityTargetImpl;

            #[agent_implementation]
            impl RustQuantityTarget for RustQuantityTargetImpl {
                fn new(_name: String) -> Self {
                    Self
                }
                fn echo(&self, value: Quantity<Length>) -> Quantity<Length> {
                    value
                }
                fn nested(&self, value: QuantityHolder) -> QuantityHolder {
                    value
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
            use golem_rust::{agent_definition, agent_implementation};

            macro_rules! exercise {
                ($client:ident, $agent:ident, $holder:ident) => {{
                    use $client::{$agent, $holder};
                    type Length = golem_rust::Quantity<$client::units::UnitM>;
                    let length = |mantissa: i64, scale: i32, unit: &str| Length::new(mantissa, scale, unit).unwrap();
                    let values = vec![
                        length(1500, 3, "km"),
                        length(i64::MIN, i32::MIN, "m"),
                        length(i64::MAX, i32::MAX, "cm"),
                    ];
                    let target = $agent::get("hub".to_string()).unwrap();
                    for value in &values {
                        assert_eq!(&target.echo(value.clone()).await.unwrap(), value);
                    }
                    for maybe in [Some(values[2].clone()), None] {
                        let holder = target
                            .nested($holder { primary: values[0].clone(), maybe: maybe.clone(), all: values.clone() })
                            .await
                            .unwrap();
                        assert_eq!((holder.primary, holder.maybe, holder.all), (values[0].clone(), maybe, values.clone()));
                    }
                    let rejected = Length::new(1, 0, "mi").unwrap_err();
                    assert!(rejected.to_string().contains("mi"), "{rejected}");
                    "quantities-ok".to_string()
                }};
            }

            #[agent_definition]
            pub trait RustQuantityCaller {
                fn new(name: String) -> Self;
                async fn run(&self, target: String) -> String;
            }

            struct RustQuantityCallerImpl;

            #[agent_implementation]
            impl RustQuantityCaller for RustQuantityCallerImpl {
                fn new(_name: String) -> Self {
                    Self
                }
                async fn run(&self, target: String) -> String {
                    match target.as_str() {
                        "ts" => exercise!(ts_quantity_target_guest_client, TsQuantityTarget, NestedInValue),
                        "go" => exercise!(go_quantity_target_guest_client, GoQuantityTarget, NestedInValue),
                        "scala" => exercise!(scala_quantity_target_guest_client, ScalaQuantityTarget, QuantityHolder),
                        "moonbit" => exercise!(moonbit_quantity_target_guest_client, MoonbitQuantityTarget, QuantityHolder),
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
                ts-quantity-target-guest-client = { path = "../golem-temp/bridge-sdk/rust/internal/ts-quantity-target-guest-client" }
                go-quantity-target-guest-client = { path = "../golem-temp/bridge-sdk/rust/internal/go-quantity-target-guest-client" }
                scala-quantity-target-guest-client = { path = "../golem-temp/bridge-sdk/rust/internal/scala-quantity-target-guest-client" }
                moonbit-quantity-target-guest-client = { path = "../golem-temp/bridge-sdk/rust/internal/moonbit-quantity-target-guest-client" }
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
    tsconfig["compilerOptions"]["paths"]["rust-quantity-target-guest-client"] = serde_json::json!(
        [
            "../golem-temp/bridge-sdk/ts/internal/rust-quantity-target-guest-client/rust-quantity-target-guest-client.ts"
        ]
    );
    tsconfig["include"].as_array_mut().unwrap().extend([
        serde_json::json!("src/**/*.ts"),
        serde_json::json!(
            "../golem-temp/bridge-sdk/ts/internal/rust-quantity-target-guest-client/*.ts"
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
            import { defineAgent, method, Quantity } from '@golemcloud/golem-ts-sdk';
            import { RustQuantityTarget } from 'rust-quantity-target-guest-client';

            type Length = { mantissa: bigint; scale: number; unit: string };

            const lengthMarker = Quantity({ baseUnit: 'm', allowedSuffixes: ['m', 'cm', 'km'] });
            const length = lengthMarker as unknown as z.ZodType<Length, Length>;
            const holder = z.object({ primary: length, maybe: z.optional(length), all: z.array(length) });

            export const TsQuantityTarget = defineAgent({
              name: 'TsQuantityTarget', id: { name: z.string() },
              methods: {
                echo: method({ input: { value: lengthMarker }, returns: lengthMarker }),
                nested: method({ input: { value: holder }, returns: holder }),
              },
            });

            export const TsQuantityTargetImpl = TsQuantityTarget.implement({
              init: () => ({}),
              methods: {
                echo({ value }) { return value; },
                nested({ value }) { return value; },
              },
            });

            export const TsQuantityCaller = defineAgent({
              name: 'TsQuantityCaller', id: { name: z.string() },
              methods: { run: method({ input: {}, returns: z.string() }) },
            });

            const text = (value: Length | undefined) =>
              value === undefined ? 'none' : `${value.mantissa}/${value.scale}/${value.unit}`;
            const check = (ok: boolean, message: string) => { if (!ok) throw new Error(message); };

            export const TsQuantityCallerImpl = TsQuantityCaller.implement({
              init: ({ id }) => ({ name: id.name }),
              methods: {
                async run() {
                  const km: Length = { mantissa: 1500n, scale: 3, unit: 'km' };
                  const min: Length = { mantissa: -(2n ** 63n), scale: -(2 ** 31), unit: 'm' };
                  const max: Length = { mantissa: 2n ** 63n - 1n, scale: 2 ** 31 - 1, unit: 'cm' };
                  const values = [km, min, max];
                  const target = RustQuantityTarget.get(this.name);
                  for (const value of values) {
                    check(text(await target.echo(value)) === text(value), `echo ${text(value)}`);
                  }
                  for (const maybe of [max, undefined]) {
                    const back = await target.nested({ primary: km, maybe, all: values });
                    check(text(back.primary) === text(km), 'nested primary');
                    check(text(back.maybe) === text(maybe), 'nested maybe');
                    check(back.all.map(text).join() === values.map(text).join(), 'nested all');
                  }
                  let rejection = '';
                  try {
                    await target.echo({ mantissa: 1n, scale: 0, unit: 'mi' });
                  } catch (error) {
                    rejection = String(error);
                  }
                  check(rejection.includes('mi'), `disallowed unit: ${rejection}`);
                  return 'quantities-ok';
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
        dir.join("agents/quantities/quantities.go"),
        indoc! {r#"
            package quantities

            import (
            	"github.com/golemcloud/golem/sdks/go/core/values"
            	"github.com/golemcloud/golem/sdks/go/golem"
            )

            type Length struct{}

            func (Length) BaseUnit() string { return "m" }

            func (Length) AllowedSuffixes() []string { return []string{"m", "cm", "km"} }

            type Meters = golem.Quantity[Length]

            type QuantityHolder struct {
            	Primary Meters
            	Maybe   values.Option[Meters]
            	All     []Meters
            }

            type TargetID struct{ Name string }

            type EchoIn struct{ Value Meters }

            type NestedIn struct{ Value QuantityHolder }

            var Target = golem.DefineAgent[TargetID](golem.Spec{Name: "GoQuantityTarget"})

            var (
            	Echo   = Target.Method[EchoIn, Meters]("echo")
            	Nested = Target.Method[NestedIn, QuantityHolder]("nested")
            )

            type CallerID struct{ Name string }

            var Caller = golem.DefineAgent[CallerID](golem.Spec{Name: "GoQuantityCaller"})

            var Run = Caller.Method[golem.Unit, string]("run")
        "#},
    )
    .unwrap();
    fs::write_str(
        dir.join("agents/quantities/impl/impl.go"),
        formatdoc! {r#"
            package impl

            import (
            	"errors"
            	"fmt"
            	"math"
            	"slices"
            	"strings"

            	"{module}/agents/quantities"

            	rust "golem.local/bridge/rust-quantity-target-guest-client"

            	"github.com/golemcloud/golem/sdks/go/core/values"
            	"github.com/golemcloud/golem/sdks/go/golem"
            )

            type targetState struct{{}}

            type callerState struct{{ name string }}

            type length = values.Quantity[rust.UnitM]

            var target = quantities.Target.Implement(func(quantities.TargetID) *targetState {{ return &targetState{{}} }})

            var caller = quantities.Caller.Implement(func(id quantities.CallerID) *callerState {{ return &callerState{{name: id.Name}} }})

            func check(ok bool, format string, args ...any) {{
            	if !ok {{
            		panic(fmt.Sprintf(format, args...))
            	}}
            }}

            func init() {{
            	target.Handle(quantities.Echo, func(_ *golem.Context[targetState], in quantities.EchoIn) quantities.Meters {{ return in.Value }})
            	target.Handle(quantities.Nested, func(_ *golem.Context[targetState], in quantities.NestedIn) quantities.QuantityHolder {{ return in.Value }})
            	caller.Handle(quantities.Run, func(ctx *golem.Context[callerState], _ golem.Unit) string {{
            		all := []length{{
            			{{Mantissa: 1500, Scale: 3, Unit: "km"}},
            			{{Mantissa: math.MinInt64, Scale: math.MinInt32, Unit: "m"}},
            			{{Mantissa: math.MaxInt64, Scale: math.MaxInt32, Unit: "cm"}},
            		}}
            		remote := golem.Must(rust.GetRustQuantityTarget(rust.RustQuantityTargetID{{Name: ctx.State.name}}))
            		for _, value := range all {{
            			echoed := golem.Must(remote.Echo(value))
            			check(echoed == value, "echo %+v returned %+v", value, echoed)
            		}}
            		for _, maybe := range []values.Option[length]{{values.Some(all[2]), values.None[length]()}} {{
            			back := golem.Must(remote.Nested(rust.QuantityHolder{{Primary: all[0], Maybe: maybe, All: all}}))
            			check(back.Primary == all[0], "nested primary %+v", back.Primary)
            			check(back.Maybe == maybe, "nested maybe %+v", back.Maybe)
            			check(slices.Equal(back.All, all), "nested all %+v", back.All)
            		}}
            		_, err := remote.Echo(length{{Mantissa: 1, Unit: "mi"}})
            		var rejection *golem.RemoteCallError
            		check(errors.As(err, &rejection) && rejection.Kind == golem.RemoteProtocolError && strings.Contains(err.Error(), `unit "mi"`), "disallowed unit: %v", err)
            		return "quantities-ok"
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
            &format!("{module}/agents/quantities/impl"),
        ),
    )
    .unwrap();
    fs::write_str(
        &go_mod,
        fs::read_to_string(&go_mod).unwrap()
            + indoc! {r#"

                require golem.local/bridge/rust-quantity-target-guest-client v0.0.0

                replace golem.local/bridge/rust-quantity-target-guest-client => ../golem-temp/bridge-sdk/go/internal/rust-quantity-target-guest-client
            "#},
    )
    .unwrap();
}

fn write_scala(ctx: &TestContext) {
    let scala_dir = ctx.cwd_path_join("scala/src/main/scala");
    std::fs::remove_dir_all(&scala_dir).unwrap();
    fs::create_dir_all(scala_dir.join("quantities")).unwrap();
    fs::write_str(
        scala_dir.join("quantities/QuantityAgents.scala"),
        indoc! {r#"
            package quantities

            import golem.BaseAgent
            import golem.runtime.annotations.{agentDefinition, agentImplementation, quantityUnit}
            import golem.schema.Quantity
            import golem.bridge.client.rust_quantity_target.{RustQuantityTargetClient, QuantityHolder => RustHolder, UnitM}
            import zio.blocks.schema.Schema
            import scala.concurrent.Future
            import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
            import scala.util.{Failure, Success}

            @quantityUnit("m", "cm", "km") sealed trait Length

            final case class QuantityHolder(primary: Quantity[Length], maybe: Option[Quantity[Length]], all: List[Quantity[Length]])
            object QuantityHolder {
              implicit val schema: Schema[QuantityHolder] = Schema.derived
            }

            @agentDefinition()
            trait ScalaQuantityTarget extends BaseAgent {
              class Id(val name: String)
              def echo(value: golem.schema.Quantity[Length]): golem.schema.Quantity[Length]
              def nested(value: QuantityHolder): QuantityHolder
            }

            @agentImplementation()
            final class ScalaQuantityTargetImpl(private val name: String) extends ScalaQuantityTarget {
              def echo(value: Quantity[Length]): Quantity[Length] = value
              def nested(value: QuantityHolder): QuantityHolder = value
            }

            @agentDefinition()
            trait ScalaQuantityCaller extends BaseAgent {
              class Id(val name: String)
              def run(): Future[String]
            }

            @agentImplementation()
            final class ScalaQuantityCallerImpl(private val name: String) extends ScalaQuantityCaller {
              def run(): Future[String] = {
                val values = List(
                  Quantity[UnitM](1500L, 3, "km"),
                  Quantity[UnitM](Long.MinValue, Int.MinValue, "m"),
                  Quantity[UnitM](Long.MaxValue, Int.MaxValue, "cm")
                )
                val target = RustQuantityTargetClient.get(name)
                for {
                  echoed <- Future.sequence(values.map(v => target.echo(v)))
                  withSome <- target.nested(RustHolder(values.head, Some(values(2)), values))
                  withNone <- target.nested(RustHolder(values.head, None, values))
                  rejection <- Future(target.echo(Quantity[UnitM](1L, 0, "mi"))).flatten.transform {
                    case Failure(error) => Success(String.valueOf(error.getMessage))
                    case Success(value) => Success(s"accepted $value")
                  }
                } yield {
                  require(echoed == values, s"echo $echoed")
                  require(withSome == RustHolder(values.head, Some(values(2)), values), s"nested $withSome")
                  require(withNone == RustHolder(values.head, None, values), s"nested $withNone")
                  require(rejection.contains("mi") && !rejection.startsWith("accepted"), s"disallowed unit: $rejection")
                  "quantities-ok"
                }
              }
            }
        "#},
    )
    .unwrap();
    let build_path = ctx.cwd_path_join("quantity_interop_scala.sbt");
    let mut build = fs::read_to_string(&build_path).unwrap();
    build.push_str("\n  .settings(Compile / unmanagedSourceDirectories += (LocalRootProject / baseDirectory).value / \"golem-temp/bridge-sdk/scala/internal/rust-quantity-target-guest-client/src/main/scala\")\n");
    fs::write_str(build_path, build).unwrap();
}

fn write_scala_caller(ctx: &TestContext) {
    let scala_dir = ctx.cwd_path_join("scala-caller/src/main/scala");
    std::fs::remove_dir_all(&scala_dir).unwrap();
    fs::create_dir_all(scala_dir.join("peers")).unwrap();
    fs::write_str(
        scala_dir.join("peers/PeerAgents.scala"),
        indoc! {r#"
            package peers

            import golem.BaseAgent
            import golem.runtime.annotations.{agentDefinition, agentImplementation}
            import golem.schema.Quantity
            import golem.bridge.client.scala_quantity_target.{ScalaQuantityTargetClient, QuantityHolder, UnitM}
            import scala.concurrent.Future
            import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
            import scala.util.{Failure, Success}

            @agentDefinition()
            trait ScalaPeerCaller extends BaseAgent {
              class Id(val name: String)
              def run(): Future[String]
            }

            @agentImplementation()
            final class ScalaPeerCallerImpl(private val name: String) extends ScalaPeerCaller {
              def run(): Future[String] = {
                val values = List(
                  Quantity[UnitM](1500L, 3, "km"),
                  Quantity[UnitM](Long.MinValue, Int.MinValue, "m"),
                  Quantity[UnitM](Long.MaxValue, Int.MaxValue, "cm")
                )
                val target = ScalaQuantityTargetClient.get(name)
                for {
                  echoed <- Future.sequence(values.map(v => target.echo(v)))
                  withSome <- target.nested(QuantityHolder(values.head, Some(values(2)), values))
                  withNone <- target.nested(QuantityHolder(values.head, None, values))
                  rejection <- Future(target.echo(Quantity[UnitM](1L, 0, "mi"))).flatten.transform {
                    case Failure(error) => Success(String.valueOf(error.getMessage))
                    case Success(value) => Success(s"accepted $value")
                  }
                } yield {
                  require(echoed == values, s"echo $echoed")
                  require(withSome == QuantityHolder(values.head, Some(values(2)), values), s"nested $withSome")
                  require(withNone == QuantityHolder(values.head, None, values), s"nested $withNone")
                  require(rejection.contains("mi") && !rejection.startsWith("accepted"), s"disallowed unit: $rejection")
                  "quantities-ok"
                }
              }
            }
        "#},
    )
    .unwrap();
    let build_path = ctx.cwd_path_join("quantity_interop_scala_caller.sbt");
    let mut build = fs::read_to_string(&build_path).unwrap();
    build.push_str("\n  .settings(Compile / unmanagedSourceDirectories += (LocalRootProject / baseDirectory).value / \"golem-temp/bridge-sdk/scala/internal/scala-quantity-target-guest-client/src/main/scala\")\n");
    fs::write_str(build_path, build).unwrap();
}

fn write_moonbit(ctx: &TestContext) {
    let manifest = ctx.cwd_path_join("moon.mod.json");
    let mut module: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
    module["deps"]["rust-quantity-target-guest-client"] = serde_json::json!({"path": "golem-temp/bridge-sdk/moonbit/internal/rust-quantity-target-guest-client"});
    fs::write_str(
        &manifest,
        serde_json::to_string_pretty(&module).unwrap() + "\n",
    )
    .unwrap();
    let manifest = ctx.cwd_path_join("moonbit/moon.pkg");
    let package = fs::read_to_string(&manifest).unwrap().replace(
        "import {",
        "import {\n  \"rust-quantity-target-guest-client/client\" @rust,",
    );
    fs::write_str(&manifest, package).unwrap();
    fs::write_str(
        ctx.cwd_path_join("moonbit/counter.mbt"),
        indoc! {r#"
            ///|
            #warnings("-struct_never_constructed")
            pub(all) struct Length {} derive(Debug, Eq)

            ///|
            pub impl @schema.QuantityUnit for Length with fn type_id() {
              "quantity_interop.Length"
            }

            ///|
            pub impl @schema.QuantityUnit for Length with fn base_unit() {
              "m"
            }

            ///|
            pub impl @schema.QuantityUnit for Length with fn allowed_suffixes() {
              ["m", "cm", "km"]
            }

            ///|
            #derive.golem_schema
            pub(all) struct QuantityHolder {
              primary : @schema.Quantity[Length]
              maybe : @schema.Quantity[Length]?
              all : Array[@schema.Quantity[Length]]
            }

            ///|
            #derive.agent
            struct MoonbitQuantityTarget {
              name : String
            }

            ///|
            fn MoonbitQuantityTarget::new(name : String) -> MoonbitQuantityTarget {
              { name, }
            }

            ///|
            pub fn MoonbitQuantityTarget::echo(
              _self : Self,
              value : @schema.Quantity[Length],
            ) -> @schema.Quantity[Length] {
              value
            }

            ///|
            pub fn MoonbitQuantityTarget::nested(_self : Self, value : QuantityHolder) -> QuantityHolder {
              value
            }

            ///|
            #derive.agent
            struct MoonbitQuantityCaller {
              name : String
            }

            ///|
            fn MoonbitQuantityCaller::new(name : String) -> MoonbitQuantityCaller {
              { name, }
            }

            ///|
            fn length(mantissa : Int64, scale : Int, unit : String) -> @schema.Quantity[@rust.UnitM] {
              @schema.Quantity::new(mantissa, scale, unit) catch {
                _ => abort("unit \{unit} rejected")
              }
            }

            ///|
            fn rejects_miles() -> Bool {
              let _ : @schema.Quantity[@rust.UnitM] = @schema.Quantity::new(1L, 0, "mi") catch {
                _ => return true
              }
              false
            }

            ///|
            pub async fn MoonbitQuantityCaller::run(self : Self) -> String {
              let values = [
                length(1500L, 3, "km"),
                length(-9223372036854775807L - 1L, -2147483648, "m"),
                length(9223372036854775807L, 2147483647, "cm"),
              ]
              @rust.RustQuantityTargetClient::scoped(self.name, async fn(remote) {
                for value in values {
                  assert_eq(remote.echo(value), value)
                }
                for maybe in [Some(values[2]), None] {
                  let back = remote.nested({ primary: values[0], maybe, all: values })
                  assert_eq(back.primary, values[0])
                  assert_eq(back.maybe, maybe)
                  assert_eq(back.all, values)
                }
                assert_true(rejects_miles())
                "quantities-ok"
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
        ("scala", "scala-caller"),
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
    write_scala_caller(&ctx);
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

async fn run_external_drivers(ctx: &TestContext) {
    let url = ctx.worker_service_url();
    let token = golem_client::LOCAL_WELL_KNOWN_TOKEN;

    for language in ["rust", "typescript", "scala", "go", "moonbit"] {
        let root = ctx.cwd_path_join(format!("{language}-quantity-bridge"));
        let output = ctx
            .cli([
                flag::YES,
                cmd::GENERATE_BRIDGE,
                flag::LANGUAGE,
                language,
                flag::AGENT_TYPE_NAME,
                "RustQuantityTarget",
                flag::OUTPUT_DIR,
                root.to_str().unwrap(),
            ])
            .await;
        assert!(output.success_or_dump());
    }

    let rust_client = ctx.cwd_path_join("rust-quantity-bridge/rust-quantity-target-client");
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
            use rust_quantity_target_client::{{configure, units::UnitM, GolemServer, Quantity, QuantityHolder, RustQuantityTarget}};

            #[tokio::main]
            async fn main() -> Result<(), Box<dyn std::error::Error>> {{
                configure(
                    GolemServer::Custom {{ url: "{url}".parse()?, token: "{token}".to_string() }},
                    "{APP}",
                    "local",
                );
                let length = |mantissa: i64, scale: i32, unit: &str| Quantity::<UnitM>::new(mantissa, scale, unit).unwrap();
                let values = vec![length(1500, 3, "km"), length(i64::MIN, i32::MIN, "m"), length(i64::MAX, i32::MAX, "cm")];
                let target = RustQuantityTarget::get("external-rust".to_string()).await?;
                for value in &values {{
                    assert_eq!(&target.echo(value.clone()).await?, value);
                }}
                for maybe in [Some(values[2].clone()), None] {{
                    let holder = target.nested(QuantityHolder {{ primary: values[0].clone(), maybe: maybe.clone(), all: values.clone() }}).await?;
                    assert_eq!((holder.primary, holder.maybe, holder.all), (values[0].clone(), maybe, values.clone()));
                }}
                let rejected = Quantity::<UnitM>::new(1, 0, "mi").unwrap_err();
                assert!(rejected.contains("mi"), "{{rejected}}");
                println!("QUANTITY_E2E_OK");
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
            ctx.cwd_path_join("rust-quantity-bridge-target"),
        )
        .current_dir(&rust_client);
    assert_generated_driver(
        run_generated_driver(rust_command).await,
        "Rust",
        "QUANTITY_E2E_OK",
    );

    let typescript_client =
        ctx.cwd_path_join("typescript-quantity-bridge/rust-quantity-target-client");
    std::fs::write(
        typescript_client.join("e2e.ts"),
        formatdoc! {r#"
            import assert from 'node:assert/strict'
            import {{ configure, RustQuantityTarget }} from './rust-quantity-target-client.js'

            configure({{
              server: {{ type: 'custom', url: '{url}', token: '{token}' }},
              application: '{APP}',
              environment: 'local',
            }})
            const values = [
              {{ mantissa: 1500n, scale: 3, unit: 'km' }},
              {{ mantissa: -(2n ** 63n), scale: -(2 ** 31), unit: 'm' }},
              {{ mantissa: 2n ** 63n - 1n, scale: 2 ** 31 - 1, unit: 'cm' }},
            ]
            const target = await RustQuantityTarget.get('external-typescript')
            for (const value of values) assert.deepEqual(await target.echo(value), value)
            for (const maybe of [values[2], undefined]) {{
              const holder = await target.nested({{ primary: values[0], maybe, all: values }})
              assert.deepEqual(holder.primary, values[0])
              assert.deepEqual(holder.maybe, maybe)
              assert.deepEqual(holder.all, values)
            }}
            await assert.rejects(target.echo({{ mantissa: 1n, scale: 0, unit: 'mi' }}), /mi/)
            console.log('QUANTITY_E2E_OK')
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
        "QUANTITY_E2E_OK",
    );

    let scala_client = ctx.cwd_path_join("scala-quantity-bridge/rust-quantity-target-client");
    std::fs::write(
        scala_client.join("src/main/scala/Main.scala"),
        formatdoc! {r#"
            import golem.bridge.client.rust_quantity_target.{{RustQuantityTargetClient, QuantityHolder, UnitM}}
            import golem.bridge.runtime.{{GolemServer, Quantity}}
            import scala.concurrent.{{Await, ExecutionContext, Future}}
            import scala.concurrent.duration.*
            import scala.util.Try

            object Main {{
              given ExecutionContext = ExecutionContext.global
              val timeout = 90.seconds

              def main(args: Array[String]): Unit = {{
                RustQuantityTargetClient.configure(GolemServer.Custom("{url}", "{token}"), "{APP}", "local")
                val values = List(
                  Quantity[UnitM](1500L, 3, "km"),
                  Quantity[UnitM](Long.MinValue, Int.MinValue, "m"),
                  Quantity[UnitM](Long.MaxValue, Int.MaxValue, "cm")
                )
                val target = Await.result(RustQuantityTargetClient.get("external-scala"), timeout)
                for (value <- values) assert(Await.result(target.echo(value), timeout) == value)
                for (maybe <- List(Some(values(2)), None)) {{
                  val holder = QuantityHolder(values.head, maybe, values)
                  assert(Await.result(target.nested(holder), timeout) == holder)
                }}
                val rejected = Try(Await.result(Future(target.echo(Quantity[UnitM](1L, 0, "mi"))).flatten, timeout))
                assert(rejected.failed.toOption.exists(_.getMessage.contains("mi")), s"disallowed unit: $rejected")
                println("QUANTITY_E2E_OK")
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
        "QUANTITY_E2E_OK",
    );

    let go_program = ctx.cwd_path_join("go-quantity-e2e");
    std::fs::create_dir_all(&go_program).unwrap();
    let sdks = workspace_path().join("sdks/go");
    std::fs::write(
        go_program.join("go.mod"),
        formatdoc! {r#"
            module example.com/go-quantity-e2e

            go {go}

            require (
            	golem.local/bridge/rust-quantity-target-client v0.0.0
            	github.com/golemcloud/golem/sdks/go/bridge v0.0.0
            	github.com/golemcloud/golem/sdks/go/core v0.0.0
            )

            replace golem.local/bridge/rust-quantity-target-client => {client}

            replace github.com/golemcloud/golem/sdks/go/bridge => {bridge}

            replace github.com/golemcloud/golem/sdks/go/core => {core}
            "#,
            go = versions::build_tool::GO_MIN,
            client = ctx.cwd_path_join("go-quantity-bridge/rust-quantity-target-client").display(),
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
            	"errors"
            	"fmt"
            	"log"
            	"math"
            	"slices"
            	"strings"

            	"github.com/golemcloud/golem/sdks/go/bridge"
            	"github.com/golemcloud/golem/sdks/go/core/values"
            	client "golem.local/bridge/rust-quantity-target-client"
            )

            type length = values.Quantity[client.UnitM]

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
            	all := []length{{
            		{{Mantissa: 1500, Scale: 3, Unit: "km"}},
            		{{Mantissa: math.MinInt64, Scale: math.MinInt32, Unit: "m"}},
            		{{Mantissa: math.MaxInt64, Scale: math.MaxInt32, Unit: "cm"}},
            	}}
            	target := must(client.GetRustQuantityTarget(client.RustQuantityTargetID{{Name: "external-go"}}))
            	for _, value := range all {{
            		if echoed := must(target.Echo(ctx, value)); echoed != value {{
            			log.Fatalf("echo %+v returned %+v", value, echoed)
            		}}
            	}}
            	for _, maybe := range []values.Option[length]{{values.Some(all[2]), values.None[length]()}} {{
            		back := must(target.Nested(ctx, client.QuantityHolder{{Primary: all[0], Maybe: maybe, All: all}}))
            		if back.Primary != all[0] || back.Maybe != maybe || !slices.Equal(back.All, all) {{
            			log.Fatalf("nested returned %+v", back)
            		}}
            	}}
            	var rejection *bridge.EncodeError
            	if _, err := target.Echo(ctx, length{{Mantissa: 1, Unit: "mi"}}); !errors.As(err, &rejection) || !strings.Contains(err.Error(), `unit "mi"`) {{
            		log.Fatalf("disallowed unit: %v", err)
            	}}
            	fmt.Println("QUANTITY_E2E_OK")
            }}
        "#},
    )
    .unwrap();
    let toolchain = golem_cli::app::build::go_toolchain::ensure_go_toolchain(
        &golem_cli::model::app::ApplicationConfig {
            offline: false,
            dev_mode: false,
            should_colorize: false,
            enable_wasmtime_fs_cache: false,
        },
    )
    .await
    .expect("the Golem Go toolchain");
    let mut go_command = std::process::Command::new(&toolchain.go);
    go_command
        .args(["run", "."])
        .current_dir(&go_program)
        .env("GOTOOLCHAIN", "local")
        .env("GOFLAGS", "-mod=mod");
    assert_generated_driver(
        run_generated_driver(go_command).await,
        "Go",
        "QUANTITY_E2E_OK",
    );

    let moonbit_client = ctx.cwd_path_join("moonbit-quantity-bridge/rust-quantity-target-client");
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
            fn length(mantissa : Int64, scale : Int, unit : String) -> @runtime.Quantity[@client.UnitM] {{
              @runtime.Quantity::new(mantissa, scale, unit) catch {{
                _ => abort("unit " + unit + " rejected")
              }}
            }}

            fn rejects_miles() -> Bool {{
              let _ : @runtime.Quantity[@client.UnitM] = @runtime.Quantity::new(1L, 0, "mi") catch {{
                _ => return true
              }}
              false
            }}

            async fn main {{
              @client.RustQuantityTarget::configure(@runtime.Custom("{url}", "{token}"), "{APP}", "local")
              let values = [
                length(1500L, 3, "km"),
                length(-9223372036854775807L - 1L, -2147483648, "m"),
                length(9223372036854775807L, 2147483647, "cm"),
              ]
              let target = @client.RustQuantityTarget::get("external-moonbit")
              for value in values {{
                assert_eq(target.echo(value), value)
              }}
              for maybe in [Some(values[2]), None] {{
                let holder = target.nested({{ primary: values[0], maybe, all: values }})
                assert_eq(holder.primary, values[0])
                assert_eq(holder.maybe, maybe)
                assert_eq(holder.all, values)
              }}
              assert_true(rejects_miles())
              println("QUANTITY_E2E_OK")
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
        "QUANTITY_E2E_OK",
    );
}

#[test]
#[timeout("60 minutes")]
async fn quantity_values_cross_every_bridge() {
    let ctx = deployed_context().await;

    for (caller, label) in [
        ("TsQuantityCaller(\"ts\")", "ts"),
        ("GoQuantityCaller(\"go\")", "go"),
        ("ScalaQuantityCaller(\"scala\")", "scala"),
        ("MoonbitQuantityCaller(\"moonbit\")", "moonbit"),
    ] {
        let stdout = invoke(&ctx, caller, &["run"]).await;
        assert!(
            stdout.contains("quantities-ok"),
            "{label} guest caller of RustQuantityTarget: {stdout}"
        );
    }

    for target in ["ts", "go", "scala", "moonbit"] {
        let stdout = invoke(
            &ctx,
            "RustQuantityCaller(\"hub\")",
            &["run", &format!("\"{target}\"")],
        )
        .await;
        assert!(
            stdout.contains("quantities-ok"),
            "Rust guest caller of the {target} target: {stdout}"
        );
    }

    let stdout = invoke(&ctx, "ScalaPeerCaller(\"peer\")", &["run"]).await;
    assert!(
        stdout.contains("quantities-ok"),
        "Scala guest caller of the Scala target: {stdout}"
    );

    run_external_drivers(&ctx).await;
}
