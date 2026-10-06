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

//! Go bridge generation, checked with the Go toolchain itself.
//!
//! Every generated client is put through three checks, each catching what the
//! others cannot:
//!
//! - `gofmt -l` must print nothing, which is how a generated file is told apart
//!   from an edited one;
//! - `go vet` must pass, for wasip1 for a guest client and natively for an
//!   external one — the targets each is built for;
//! - a native `go test` round-trips a value of every generated type. For a guest
//!   client that goes through the SDK's own codec, which records a malformed
//!   registration — a variant case that does not implement its interface, an
//!   enum over the wrong kind — as a definition error at run time, where neither
//!   of the other checks looks. For an external client it goes through the
//!   generated conversions and the REST wire form, and the client itself is
//!   driven against a recording server.
//!
//! The toolchain is the one the CLI builds Go components with, resolved through
//! `ensure_go_toolchain` rather than whatever `go` is on PATH, which can be older
//! than the generated code needs. Go's build and module caches are safe for
//! concurrent use, so the suite runs in parallel.

use crate::bridge_gen::fixtures::{
    agent, def, field, local_config, method, multimodal, named_field, ref_to, variant_case,
};
use crate::bridge_gen::scala::{command_node, doc, grep_tool, option, positional, tool_body};
use camino::{Utf8Path, Utf8PathBuf};
use golem_cli::app::build::go_toolchain::{GoToolchain, ensure_go_toolchain};
use golem_cli::bridge_gen::BridgeGenerator;
use golem_cli::bridge_gen::go::tool::GoToolBridgeGenerator;
use golem_cli::bridge_gen::go::{GoBridgeGenerator, GoBridgeMode};
use golem_cli::model::app::ApplicationConfig;
use golem_cli::sdk_overrides::workspace_root;
use golem_common::model::agent::{
    AgentConfigSource, AgentMode, CorsOptions, FileMapping, HttpMountDetails,
};
use golem_common::schema::agent::AgentConfigDeclarationSchema;
use golem_common::schema::schema_type::{
    DiscriminatorRule, NumericBound, NumericRestrictions, PathDirection, PathKind, PathSpec,
    QuantitySpec, ResultSpec, TextRestrictions, UnionBranch, UnionSpec,
};
use golem_common::schema::schema_value::SchemaValue;
use golem_common::schema::tool::{
    CommandBody, CommandIndex, DuplicateKeyPolicy, ErrorCase, ErrorKind, Formatter, OptionShape,
    OptionSpec, Positional, Positionals, RepeatableMapShape, Repetition,
    ResultSpec as ToolResultSpec, StreamSpec, Tool,
};
use golem_common::schema::{AgentTypeSchema, AutoInjectedKind, NamedField, Role, SchemaType};
use std::process::Command;
use tempfile::TempDir;
use test_r::{test, test_dep};

/// The Go toolchain and the caches every check shares.
pub struct GoEnv {
    toolchain: GoToolchain,
    cache_dir: Utf8PathBuf,
}

impl GoEnv {
    fn command(&self, dir: &Utf8Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(&self.toolchain.go);
        cmd.args(args)
            .current_dir(dir)
            // The pinned toolchain, and nothing else: GOTOOLCHAIN=auto would
            // re-exec whatever a go directive asks for.
            .env("GOTOOLCHAIN", "local")
            .env("GOCACHE", self.cache_dir.join("build"))
            .env("GOMODCACHE", self.cache_dir.join("mod"))
            .env("GOFLAGS", "-mod=mod");
        cmd
    }

    fn gofmt(&self) -> std::path::PathBuf {
        self.toolchain.bin_dir.join("gofmt")
    }
}

#[test_dep]
async fn go_env() -> GoEnv {
    let toolchain = ensure_go_toolchain(&ApplicationConfig {
        offline: false,
        dev_mode: false,
        should_colorize: false,
        enable_wasmtime_fs_cache: false,
    })
    .await
    .expect("the Golem Go toolchain");
    let root = workspace_root().expect("the workspace root");
    let cache_dir = Utf8PathBuf::from_path_buf(root.join("target/shared_bridge_tests/go"))
        .expect("a UTF-8 cache path");
    std::fs::create_dir_all(&cache_dir).expect("the shared Go cache directory");
    GoEnv {
        toolchain,
        cache_dir,
    }
}

/// A generated client, in its own temporary module.
struct GeneratedGo {
    dir: TempDir,
}

impl GeneratedGo {
    fn guest(env: &GoEnv, agent_type: AgentTypeSchema) -> Self {
        let dir = TempDir::new().unwrap();
        let target = Utf8Path::from_path(dir.path()).unwrap();
        let mut generator =
            GoBridgeGenerator::new_with_mode(agent_type, target, GoBridgeMode::GuestWasmRpc)
                .expect("a generator");
        generator.generate().expect("generation");
        let generated = Self { dir };
        generated.point_at_the_workspace_sdk();
        generated.run(env, &["mod", "tidy"]);
        generated
    }

    fn tool(env: &GoEnv, tool: Tool) -> Self {
        let dir = TempDir::new().unwrap();
        let target = Utf8Path::from_path(dir.path()).unwrap();
        GoToolBridgeGenerator::new(tool, target, true)
            .expect("a generator")
            .generate()
            .expect("generation");
        let generated = Self { dir };
        generated.point_at_the_workspace_sdk();
        generated.run(env, &["mod", "tidy"]);
        generated
    }

    fn external(env: &GoEnv, agent_type: AgentTypeSchema) -> Self {
        let dir = TempDir::new().unwrap();
        let target = Utf8Path::from_path(dir.path()).unwrap();
        let mut generator =
            GoBridgeGenerator::new_with_mode(agent_type, target, GoBridgeMode::ExternalRest)
                .expect("a generator");
        generator.generate().expect("generation");
        let generated = Self { dir };
        generated.point_at_the_workspace_sdk();
        generated.run(env, &["mod", "tidy"]);
        generated
    }

    fn package(&self) -> String {
        self.read("client.go")
            .lines()
            .find_map(|l| l.strip_prefix("package ").map(str::to_string))
            .expect("a package clause")
    }

    fn path(&self) -> &Utf8Path {
        Utf8Path::from_path(self.dir.path()).unwrap()
    }

    fn read(&self, file: &str) -> String {
        std::fs::read_to_string(self.path().join(file)).unwrap()
    }

    /// Resolves the SDK from this checkout, as a consuming component's go.mod
    /// would: a replace in the client's own go.mod is ignored once it is a
    /// dependency, but it is what lets the client build on its own here.
    fn point_at_the_workspace_sdk(&self) {
        let root = workspace_root().unwrap();
        let go_mod = self.path().join("go.mod");
        let mut content = std::fs::read_to_string(&go_mod).unwrap();
        if !content.contains("replace ") {
            for (module, dir) in [
                ("github.com/golemcloud/golem/sdks/go/golem", "sdks/go/golem"),
                ("github.com/golemcloud/golem/sdks/go/core", "sdks/go/core"),
                (
                    "github.com/golemcloud/golem/sdks/go/bridge",
                    "sdks/go/bridge",
                ),
            ] {
                if !content.contains(module) {
                    continue;
                }
                content.push_str(&format!(
                    "\nreplace {module} => {}\n",
                    root.join(dir).display()
                ));
            }
            std::fs::write(&go_mod, content).unwrap();
        }
    }

    fn run(&self, env: &GoEnv, args: &[&str]) -> String {
        self.run_with(env, args, &[])
    }

    fn run_with(&self, env: &GoEnv, args: &[&str], vars: &[(&str, &str)]) -> String {
        let mut cmd = env.command(self.path(), args);
        for (key, value) in vars {
            cmd.env(key, value);
        }
        let output = cmd.output().expect("the go command runs");
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        assert!(
            output.status.success(),
            "go {} failed in {}:\n{stdout}\n{stderr}",
            args.join(" "),
            self.path()
        );
        stdout
    }

    /// Fails with gofmt's own diff, so a failure shows what to change in the
    /// generator rather than just which file drifted.
    fn assert_gofmt_clean(&self, env: &GoEnv) {
        let output = Command::new(env.gofmt())
            .args(["-d", "."])
            .current_dir(self.path())
            .output()
            .expect("gofmt runs");
        let diff = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && diff.trim().is_empty(),
            "gofmt would rewrite the generated code:\n{diff}{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn assert_vets_for_wasip1(&self, env: &GoEnv) {
        self.run_with(
            env,
            &["vet", "./..."],
            &[("GOOS", "wasip1"), ("GOARCH", "wasm")],
        );
    }

    fn assert_vets_natively(&self, env: &GoEnv) {
        self.run(env, &["vet", "./..."]);
    }

    /// Drops a test into the generated package and runs it natively.
    fn run_native_test(&self, env: &GoEnv, source: &str) {
        std::fs::write(self.path().join("zz_generated_check_test.go"), source).unwrap();
        self.run(env, &["test", "./..."]);
    }
}

fn counter_agent() -> AgentTypeSchema {
    agent(
        "CounterAgent",
        "rust",
        vec![field("name", SchemaType::string())],
        vec![
            method("increment", vec![], Some(SchemaType::f64())),
            method("add", vec![field("by", SchemaType::u32())], None),
        ],
        vec![],
        AgentMode::Durable,
    )
}

/// Every schema type the Go bridge spells, each reached from a method so it
/// lands in the generated package.
fn kitchen_sink_agent() -> AgentTypeSchema {
    let order = SchemaType::record(vec![
        named_field("order-id", SchemaType::string()),
        named_field("placed-at", SchemaType::datetime()),
        named_field(
            "tags",
            SchemaType::map(SchemaType::string(), SchemaType::u32()),
        ),
        named_field("lines", SchemaType::list(SchemaType::s64())),
        named_field("digest", SchemaType::fixed_list(SchemaType::u8(), 4)),
    ]);
    let status = SchemaType::r#enum(vec!["pending".into(), "in-transit".into()]);
    let perms = SchemaType::flags(vec!["read".into(), "write-all".into()]);
    let event = SchemaType::variant(vec![
        variant_case("at", Some(SchemaType::datetime())),
        variant_case("note", Some(SchemaType::text(Default::default()))),
        variant_case("maybe", Some(SchemaType::option(SchemaType::string()))),
        variant_case("status", Some(ref_to("shop.Status"))),
        variant_case("cleared", None),
    ]);
    let handle = SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "user".into(),
                body: SchemaType::string(),
                discriminator: DiscriminatorRule::Prefix { prefix: "@".into() },
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "team".into(),
                body: SchemaType::string(),
                discriminator: DiscriminatorRule::Prefix { prefix: "#".into() },
                metadata: Default::default(),
            },
        ],
    });
    agent(
        "KitchenSink",
        "rust",
        vec![field("tenant", SchemaType::string())],
        vec![
            method(
                "place",
                vec![field("order", ref_to("shop.Order"))],
                Some(ref_to("shop.Status")),
            ),
            method("grant", vec![field("perms", ref_to("shop.Perms"))], None),
            method(
                "record",
                vec![field("event", ref_to("shop.Event"))],
                Some(SchemaType::bool()),
            ),
            method(
                "resolve",
                vec![field("handle", ref_to("shop.Handle"))],
                Some(SchemaType::string()),
            ),
            method(
                "summarise",
                vec![
                    field("since", SchemaType::duration()),
                    field(
                        "pair",
                        SchemaType::tuple(vec![SchemaType::string(), SchemaType::char()]),
                    ),
                ],
                Some(SchemaType::result(ResultSpec {
                    ok: Some(Box::new(SchemaType::u64())),
                    err: Some(Box::new(SchemaType::string())),
                })),
            ),
        ],
        vec![
            def("shop.Order", order),
            def("shop.Status", status),
            def("shop.Perms", perms),
            def("shop.Event", event),
            def("shop.Handle", handle),
        ],
        AgentMode::Durable,
    )
}

#[test_dep(tagged_as = "go_guest_counter")]
fn go_guest_counter(env: &GoEnv) -> GeneratedGo {
    GeneratedGo::guest(env, counter_agent())
}

#[test_dep(tagged_as = "go_guest_kitchen_sink")]
fn go_guest_kitchen_sink(env: &GoEnv) -> GeneratedGo {
    GeneratedGo::guest(env, kitchen_sink_agent())
}

#[test]
fn go_guest_counter_is_gofmt_clean_and_vets(
    env: &GoEnv,
    #[tagged_as("go_guest_counter")] generated: &GeneratedGo,
) {
    generated.assert_gofmt_clean(env);
    generated.assert_vets_for_wasip1(env);
}

#[test]
fn go_guest_counter_has_a_typed_client(#[tagged_as("go_guest_counter")] generated: &GeneratedGo) {
    let client = generated.read("client.go");
    assert!(
        client.contains("type CounterAgentId struct {\n\tName string\n}"),
        "{client}"
    );
    assert!(
        client.contains(
            "golem.DefineFullAgentClient[CounterAgentId](\"CounterAgent\", golem.AgentClientSpec{})"
        ),
        "{client}"
    );
    // A method without parameters takes golem.Unit, as a hand-written one would.
    assert!(
        client.contains("Method[golem.Unit, float64](\"increment\")"),
        "{client}"
    );
    assert!(!client.contains("CounterAgentIncrementInput"), "{client}");
    // A method with no output returns nothing; one with an output returns it.
    assert!(
        client.contains("func (c CounterAgentClient) Increment() float64 {"),
        "{client}"
    );
    assert!(
        client.contains("func (c CounterAgentClient) Add(by uint32) {"),
        "{client}"
    );
    let go_mod = generated.read("go.mod");
    assert!(
        go_mod.contains("module golem.local/bridge/counter-agent-guest-client"),
        "{go_mod}"
    );
}

#[test]
fn go_guest_kitchen_sink_is_gofmt_clean_and_vets(
    env: &GoEnv,
    #[tagged_as("go_guest_kitchen_sink")] generated: &GeneratedGo,
) {
    generated.assert_gofmt_clean(env);
    generated.assert_vets_for_wasip1(env);
}

/// The registrations are only checked at run time, so every generated type is
/// put through the SDK's codec: a malformed one fails here as a definition
/// error rather than on the first real call.
#[test]
fn go_guest_kitchen_sink_types_round_trip_through_the_sdk(
    env: &GoEnv,
    #[tagged_as("go_guest_kitchen_sink")] generated: &GeneratedGo,
) {
    let package = generated
        .read("types.go")
        .lines()
        .find_map(|l| l.strip_prefix("package ").map(str::to_string))
        .expect("a package clause");
    generated.run_native_test(
        env,
        &format!(
            r#"package {package}

import (
	"reflect"
	"testing"
	"time"

	"github.com/golemcloud/golem/sdks/go/core/values"
	"github.com/golemcloud/golem/sdks/go/golem"
)

func roundTrip[T any](t *testing.T, name string, in T) {{
	t.Helper()
	tv, err := golem.EncodeTypedValue(in)
	if err != nil {{
		t.Fatalf("%s: encode: %v", name, err)
	}}
	out, err := golem.DecodeTypedValue[T](tv)
	if err != nil {{
		t.Fatalf("%s: decode: %v", name, err)
	}}
	if !reflect.DeepEqual(in, out) {{
		t.Fatalf("%s: %#v round-tripped to %#v", name, in, out)
	}}
}}

func TestGeneratedTypesRoundTrip(t *testing.T) {{
	at := time.Date(2026, 9, 24, 12, 0, 0, 0, time.UTC)
	roundTrip(t, "record", ShopOrder{{
		OrderId: "o1", PlacedAt: at, Tags: map[string]uint32{{"a": 1}},
		Lines: []int64{{1, 2}}, Digest: [4]uint8{{1, 2, 3, 4}},
	}})
	roundTrip(t, "enum", ShopStatusInTransit)
	roundTrip(t, "flags", ShopPerms{{Read: true, WriteAll: true}})
	roundTrip(t, "variant, datetime", ShopEvent(ShopEventAt{{Value: at}}))
	roundTrip(t, "variant, text", ShopEvent(ShopEventNote{{Value: "hi"}}))
	roundTrip(t, "variant, option", ShopEvent(ShopEventMaybe{{Value: values.Some("x")}}))
	roundTrip(t, "variant, enum", ShopEvent(ShopEventStatus{{Value: ShopStatusPending}}))
	roundTrip(t, "variant, no payload", ShopEvent(ShopEventCleared{{}}))
	roundTrip(t, "union", ShopHandle(ShopHandleUser{{Value: "@ada"}}))
	roundTrip(t, "tuple", values.Tuple2[string, values.Char]{{A: "x", B: 'y'}})
	roundTrip(t, "result", values.Ok[uint64, string](7))
	if got := ShopStatusInTransit.String(); got != "in-transit" {{
		t.Fatalf("String() = %q", got)
	}}
}}
"#
        ),
    );
}

/// A named type that collides with a name the generator emits for the agent
/// itself is renamed away from it, since Go has one namespace per package.
#[test]
fn go_guest_reserves_the_agent_level_names(env: &GoEnv) {
    let colliding = agent(
        "CounterAgent",
        "rust",
        vec![field("id", ref_to("CounterAgentId"))],
        vec![method("get", vec![], Some(ref_to("CounterAgentId")))],
        vec![def(
            "CounterAgentId",
            SchemaType::record(vec![named_field("value", SchemaType::string())]),
        )],
        AgentMode::Durable,
    );
    let generated = GeneratedGo::guest(env, colliding);
    generated.assert_vets_for_wasip1(env);
}

#[test_dep(tagged_as = "go_external_counter")]
fn go_external_counter(env: &GoEnv) -> GeneratedGo {
    GeneratedGo::external(env, counter_agent())
}

#[test_dep(tagged_as = "go_external_kitchen_sink")]
fn go_external_kitchen_sink(env: &GoEnv) -> GeneratedGo {
    GeneratedGo::external(env, kitchen_sink_agent())
}

#[test]
fn go_external_counter_is_gofmt_clean_and_vets(
    env: &GoEnv,
    #[tagged_as("go_external_counter")] generated: &GeneratedGo,
) {
    generated.assert_gofmt_clean(env);
    generated.assert_vets_natively(env);
}

/// An external client depends on the bridge runtime and core only: pulling in
/// the guest SDK would drag its WebAssembly bindings into an ordinary program.
#[test]
fn go_external_client_does_not_depend_on_the_guest_sdk(
    #[tagged_as("go_external_counter")] generated: &GeneratedGo,
) {
    let go_mod = generated.read("go.mod");
    assert!(
        go_mod.contains("module golem.local/bridge/counter-agent-client"),
        "{go_mod}"
    );
    assert!(
        go_mod.contains("github.com/golemcloud/golem/sdks/go/bridge"),
        "{go_mod}"
    );
    assert!(
        !go_mod.contains("github.com/golemcloud/golem/sdks/go/golem "),
        "{go_mod}"
    );
    for file in ["types.go", "codec.go", "client.go"] {
        let source = generated.read(file);
        assert!(!source.contains("sdks/go/golem\""), "{file}:\n{source}");
    }
}

#[test]
fn go_external_kitchen_sink_is_gofmt_clean_and_vets(
    env: &GoEnv,
    #[tagged_as("go_external_kitchen_sink")] generated: &GeneratedGo,
) {
    generated.assert_gofmt_clean(env);
    generated.assert_vets_natively(env);
}

/// Every generated conversion, checked against what actually travels: encode,
/// marshal to the REST wire form, unmarshal, decode.
#[test]
fn go_external_kitchen_sink_types_round_trip_through_the_wire(
    env: &GoEnv,
    #[tagged_as("go_external_kitchen_sink")] generated: &GeneratedGo,
) {
    let package = generated.package();
    generated.run_native_test(
        env,
        &format!(
            r##"package {package}

import (
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/core/values"
)

func roundTrip[T any](t *testing.T, name string, in T, enc func(T) schema.SchemaValue, dec func(schema.SchemaValue) (T, error)) {{
	t.Helper()
	data, err := schema.MarshalWireValue(enc(in))
	if err != nil {{
		t.Fatalf("%s: marshal: %v", name, err)
	}}
	sv, err := schema.UnmarshalWireValue(data)
	if err != nil {{
		t.Fatalf("%s: unmarshal: %v", name, err)
	}}
	out, err := dec(sv)
	if err != nil {{
		t.Fatalf("%s: decode: %v", name, err)
	}}
	if !reflect.DeepEqual(in, out) {{
		t.Fatalf("%s: %#v round-tripped to %#v", name, in, out)
	}}
}}

func TestGeneratedConversionsRoundTrip(t *testing.T) {{
	at := time.Date(2026, 9, 24, 12, 0, 0, 0, time.UTC)
	roundTrip(t, "record", ShopOrder{{
		OrderId: "o1", PlacedAt: at, Tags: map[string]uint32{{"a": 1, "b": 2}},
		Lines: []int64{{1, -2}}, Digest: [4]uint8{{1, 2, 3, 4}},
	}}, encodeShopOrder, decodeShopOrder)
	roundTrip(t, "enum", ShopStatusInTransit, encodeShopStatus, decodeShopStatus)
	roundTrip(t, "flags", ShopPerms{{WriteAll: true}}, encodeShopPerms, decodeShopPerms)
	for name, event := range map[string]ShopEvent{{
		"datetime":   ShopEventAt{{Value: at}},
		"text":       ShopEventNote{{Value: "hi"}},
		"option":     ShopEventMaybe{{Value: values.Some("x")}},
		"none":       ShopEventMaybe{{Value: values.None[string]()}},
		"enum":       ShopEventStatus{{Value: ShopStatusPending}},
		"no payload": ShopEventCleared{{}},
	}} {{
		roundTrip(t, "variant, "+name, event, encodeShopEvent, decodeShopEvent)
	}}
	roundTrip(t, "union", ShopHandle(ShopHandleTeam{{Value: "#core"}}), encodeShopHandle, decodeShopHandle)
}}

// A value of the wrong shape names the type it was decoded as.
func TestADecodingFailureNamesTheType(t *testing.T) {{
	_, err := decodeShopOrder(schema.RecordValue{{Fields: []schema.SchemaValue{{schema.StringValue{{Value: "o1"}}}}}})
	if err == nil || !strings.Contains(err.Error(), "ShopOrder") {{
		t.Fatalf("got %v", err)
	}}
	_, err = decodeShopEvent(schema.VariantValue{{Case: 9}})
	if err == nil || !strings.Contains(err.Error(), "ShopEvent has no case 9") {{
		t.Fatalf("got %v", err)
	}}
}}
"##
        ),
    );
}

/// The client itself, against a server that records what it is sent and
/// answers as the real one would.
#[test]
fn go_external_counter_client_speaks_the_rest_protocol(
    env: &GoEnv,
    #[tagged_as("go_external_counter")] generated: &GeneratedGo,
) {
    let package = generated.package();
    generated.run_native_test(
        env,
        &format!(
            r#"package {package}

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/golemcloud/golem/sdks/go/bridge"
)

func TestTheClientInvokesTriggersAndSchedules(t *testing.T) {{
	var bodies []map[string]any
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {{
		raw, _ := io.ReadAll(r.Body)
		var body map[string]any
		_ = json.Unmarshal(raw, &body)
		bodies = append(bodies, body)
		w.Header().Set("Content-Type", "application/json")
		result := ""
		if body["methodName"] == "increment" && body["mode"] == "await" {{
			result = `,"result":{{"graph":{{"root":{{"kind":"f64","value":{{}}}}}},"value":{{"kind":"f64","value":2.5}}}}`
		}}
		_, _ = io.WriteString(w, `{{"agentId":{{"componentId":"c","agentId":"a"}},"idempotencyKey":"k"`+result+`}}`)
	}}))
	defer server.Close()

	client, err := GetCounterAgent(CounterAgentId{{Name: "c1"}}, bridge.WithConfiguration(bridge.Configuration{{
		Server: bridge.Custom(server.URL, "token"), AppName: "app", EnvName: "env",
	}}))
	if err != nil {{
		t.Fatal(err)
	}}
	ctx := context.Background()

	got, err := client.Increment(ctx)
	if err != nil || got != 2.5 {{
		t.Fatalf("Increment = %v, %v", got, err)
	}}
	if err := client.Add(ctx, 3); err != nil {{
		t.Fatal(err)
	}}
	if _, err := client.TriggerAdd(ctx, 4); err != nil {{
		t.Fatal(err)
	}}
	if _, err := client.ScheduleAdd(ctx, time.Date(2030, 1, 1, 0, 0, 0, 0, time.UTC), 5); err != nil {{
		t.Fatal(err)
	}}

	if len(bodies) != 4 {{
		t.Fatalf("%d requests", len(bodies))
	}}
	params := func(v any) string {{
		out, _ := json.Marshal(v)
		return string(out)
	}}
	if want := `{{"kind":"record","value":{{"fields":[{{"kind":"string","value":"c1"}}]}}}}`; params(bodies[0]["parameters"]) != want {{
		t.Fatalf("constructor arguments sent as %s", params(bodies[0]["parameters"]))
	}}
	if want := `{{"kind":"record","value":{{"fields":[{{"kind":"u32","value":3}}]}}}}`; params(bodies[1]["methodParameters"]) != want {{
		t.Fatalf("method arguments sent as %s", params(bodies[1]["methodParameters"]))
	}}
	for i, want := range []string{{"await", "await", "schedule", "schedule"}} {{
		if bodies[i]["mode"] != want {{
			t.Fatalf("request %d in mode %v, want %s", i, bodies[i]["mode"], want)
		}}
	}}
	if bodies[3]["scheduleAt"] != "2030-01-01T00:00:00Z" {{
		t.Fatalf("scheduled at %v", bodies[3]["scheduleAt"])
	}}
}}
"#
        ),
    );
}

/// An agent method whose name makes another's trigger name keeps its own, and
/// the trigger is renamed around it.
#[test]
fn go_external_method_names_do_not_collide(env: &GoEnv) {
    let colliding = agent(
        "Poller",
        "rust",
        vec![],
        vec![
            method("poll", vec![], None),
            method("trigger-poll", vec![], None),
            method("agent", vec![], None),
        ],
        vec![],
        AgentMode::Durable,
    );
    let generated = GeneratedGo::external(env, colliding);
    generated.assert_vets_natively(env);
    let client = generated.read("client.go");
    assert!(
        client.contains(") TriggerPoll(ctx context.Context) error {"),
        "{client}"
    );
    assert!(
        client.contains(") TriggerPoll2(ctx context.Context) (bridge.Receipt, error) {"),
        "{client}"
    );
    assert!(
        client.contains(") Agent2(ctx context.Context) error {"),
        "{client}"
    );
}

/// An agent whose methods partly take or return streams. The stream-bearing
/// ones are left out of Go clients — including a record type only they use —
/// and the client says so; the rest is generated and builds.
fn partly_streaming_agent() -> AgentTypeSchema {
    agent(
        "MediaAgent",
        "rust",
        vec![field("name", SchemaType::string())],
        vec![
            method("count", vec![], Some(SchemaType::u64())),
            method(
                "upload",
                vec![field(
                    "chunks",
                    SchemaType::stream(Some(SchemaType::list(SchemaType::u8()))),
                )],
                Some(SchemaType::u64()),
            ),
            method("feed", vec![], Some(ref_to("media.Feed"))),
        ],
        vec![def(
            "media.Feed",
            SchemaType::record(vec![named_field(
                "items",
                SchemaType::stream(Some(SchemaType::string())),
            )]),
        )],
        AgentMode::Durable,
    )
}

/// A guest client calls stream-bearing methods over RPC, so it keeps them,
/// spelling each stream as the SDK's `golem.AgentStream`.
#[test]
fn go_guest_client_generates_stream_bearing_methods(env: &GoEnv) {
    let generated = GeneratedGo::guest(env, partly_streaming_agent());
    let client = generated.read("client.go");
    assert!(client.contains(") Count("), "{client}");
    assert!(client.contains(") Upload("), "{client}");
    assert!(client.contains(") Feed("), "{client}");
    assert!(client.contains("golem.AgentStream[[]uint8]"), "{client}");
    let types = generated.read("types.go");
    assert!(types.contains("golem.AgentStream[string]"), "{types}");
    generated.assert_gofmt_clean(env);
    generated.assert_vets_for_wasip1(env);
}

/// An external client calls stream-bearing methods over an invocation
/// session, spelling each stream as the bridge's `bridge.AgentStream`. They
/// only await: there is no trigger or schedule form.
#[test]
fn go_external_client_streams_over_a_session(env: &GoEnv) {
    let generated = GeneratedGo::external(env, partly_streaming_agent());
    let client = generated.read("client.go");
    for expected in [
        ") Count(ctx context.Context) (uint64, error)",
        ") Upload(ctx context.Context, chunks bridge.AgentStream[[]uint8]) (uint64, error)",
        "return bridge.CallStreaming(ctx, c.agent, \"upload\"",
        ") Feed(ctx context.Context) (MediaFeed, error)",
        ") TriggerCount(",
    ] {
        assert!(
            client.contains(expected),
            "missing {expected} in:\n{client}"
        );
    }
    assert!(!client.contains("TriggerUpload"), "{client}");
    assert!(!client.contains("ScheduleFeed"), "{client}");
    let types = generated.read("types.go");
    assert!(types.contains("bridge.AgentStream[string]"), "{types}");
    let codec = generated.read("codec.go");
    assert!(
        codec.contains("bridge.DecodeStream(bridge.DecodeString)"),
        "{codec}"
    );
    generated.assert_gofmt_clean(env);
    generated.assert_vets_natively(env);
}

fn quantity_agent() -> AgentTypeSchema {
    let kilograms = SchemaType::Quantity {
        spec: QuantitySpec {
            base_unit: "kg".into(),
            allowed_suffixes: vec![],
            min: None,
            max: None,
        },
        metadata: Default::default(),
    };
    let lengths = SchemaType::Quantity {
        spec: QuantitySpec {
            base_unit: "m".into(),
            allowed_suffixes: vec!["m".into(), "km".into()],
            min: None,
            max: None,
        },
        metadata: Default::default(),
    };
    agent(
        "ScaleAgent",
        "rust",
        vec![field("name", SchemaType::string())],
        vec![
            method(
                "weigh",
                vec![field("load", kilograms.clone())],
                Some(kilograms),
            ),
            method("measure", vec![], Some(SchemaType::list(lengths))),
        ],
        vec![],
        AgentMode::Durable,
    )
}

/// A quantity is `values.Quantity[U]`, with one generated unit marker per
/// distinct unit.
#[test]
fn go_clients_spell_quantities_with_unit_markers(env: &GoEnv) {
    let guest = GeneratedGo::guest(env, quantity_agent());
    let types = guest.read("types.go");
    for expected in [
        "type UnitKg struct{}",
        "return \"kg\"",
        "type UnitM struct{}",
        "return []string{\"m\", \"km\"}",
    ] {
        assert!(types.contains(expected), "missing {expected} in:\n{types}");
    }
    let client = guest.read("client.go");
    assert!(client.contains("values.Quantity[UnitKg]"), "{client}");
    guest.assert_gofmt_clean(env);
    guest.assert_vets_for_wasip1(env);

    let external = GeneratedGo::external(env, quantity_agent());
    external.assert_gofmt_clean(env);
    external.assert_vets_natively(env);
    let package = external.package();
    external.run_native_test(
        env,
        &format!(
            r#"package {package}

import (
	"testing"

	"github.com/golemcloud/golem/sdks/go/bridge"
	"github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/core/values"
)

func TestQuantitiesTravelInTheirUnit(t *testing.T) {{
	sv := bridge.EncodeQuantity(values.Quantity[UnitKg]{{Mantissa: 15, Scale: 1}})
	if sv.(schema.QuantityValueNode).Value.Unit != "kg" {{
		t.Fatalf("encoded as %#v", sv)
	}}
	back, err := bridge.DecodeQuantity[UnitM](schema.QuantityValueNode{{Value: schema.QuantityValue{{Mantissa: 2, Unit: "km"}}}})
	if err != nil || back.Unit != "km" {{
		t.Fatalf("decoded %#v, %v", back, err)
	}}
	if _, err := bridge.DecodeQuantity[UnitKg](schema.QuantityValueNode{{Value: schema.QuantityValue{{Unit: "lb"}}}}); err == nil {{
		t.Fatal("a quantity in a unit it does not accept was decoded")
	}}
}}
"#
        ),
    );
}

/// The host fills a principal parameter, so neither client asks a caller for
/// one: not in the id, not in a method's arguments.
#[test]
fn go_clients_leave_out_the_principal(env: &GoEnv) {
    let principal = || {
        NamedField::auto_injected(
            "principal",
            AutoInjectedKind::Principal,
            SchemaType::record(vec![]),
        )
    };
    let with_principal = || {
        agent(
            "LedgerAgent",
            "go",
            vec![field("name", SchemaType::string()), principal()],
            vec![method(
                "charge",
                vec![field("amount", SchemaType::s64()), principal()],
                Some(SchemaType::string()),
            )],
            vec![],
            AgentMode::Durable,
        )
    };
    for generated in [
        GeneratedGo::guest(env, with_principal()),
        GeneratedGo::external(env, with_principal()),
    ] {
        let client = generated.read("client.go");
        assert!(!client.to_lowercase().contains("principal"), "{client}");
        // the guest client takes the arguments alone, the external one a ctx first
        assert!(
            client.contains("Charge(amount int64)")
                || client.contains("Charge(ctx context.Context, amount int64)"),
            "{client}"
        );
        generated.assert_gofmt_clean(env);
    }
}

/// A constructor stream cannot be left out: without it there is no client.
#[test]
fn go_client_refuses_a_constructor_stream() {
    let streaming_id = agent(
        "Tail",
        "rust",
        vec![field(
            "source",
            SchemaType::stream(Some(SchemaType::string())),
        )],
        vec![method("count", vec![], Some(SchemaType::u64()))],
        vec![],
        AgentMode::Durable,
    );
    let dir = TempDir::new().unwrap();
    let err = GoBridgeGenerator::new_with_mode(
        streaming_id,
        Utf8Path::from_path(dir.path()).unwrap(),
        GoBridgeMode::ExternalRest,
    )
    .err()
    .expect("a constructor stream is refused");
    assert!(
        err.to_string().contains("its constructor takes a stream"),
        "{err}"
    );
}

/// The shared HTTP handler corpus decides which agents get clients: an ordinary
/// agent that exposes files keeps its client, and a router never gets one —
/// decided by the agent's kind, not its name.
#[test]
fn go_http_router_bridge_rejection_uses_kind_not_name() {
    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    ))
    .unwrap();
    let case = |id: &str| {
        corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == id)
            .unwrap()
            .clone()
    };
    let regular_files = case("tooling-regular-files-still-callable");
    let router = case("tooling-router-clients");
    let dir = TempDir::new().unwrap();
    let path = Utf8Path::from_path(dir.path()).unwrap();
    for mode in [GoBridgeMode::ExternalRest, GoBridgeMode::GuestWasmRpc] {
        let mut metadata = agent(
            "HttpRouterLookingName",
            "go",
            vec![],
            vec![],
            vec![],
            AgentMode::Durable,
        );
        metadata.http_mount = Some(HttpMountDetails {
            path_prefix: vec![],
            auth_details: None,
            phantom_agent: false,
            cors_options: CorsOptions {
                allowed_patterns: vec![],
            },
            webhook_suffix: vec![],
            static_bindings: vec![],
            filesystem_bindings: FileMapping::compile_list(
                regular_files["input"]["filesystem_bindings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|mapping| (mapping[0].as_str().unwrap(), mapping[1].as_str().unwrap())),
            )
            .unwrap(),
            openapi_provider_method: None,
        });
        assert_eq!(
            GoBridgeGenerator::new_with_mode(metadata.clone(), path, mode).is_ok(),
            regular_files["expect"]["included"].as_bool().unwrap(),
            "tooling-regular-files-still-callable ({mode:?})"
        );

        metadata.type_name =
            golem_common::model::agent::AgentTypeName("OrdinaryLookingName".into());
        metadata.kind = golem_common::schema::agent::AgentTypeKind::HttpRouter;
        let result = GoBridgeGenerator::new_with_mode(metadata, path, mode);
        assert_eq!(
            result.is_ok(),
            router["expect"]["included"].as_bool().unwrap(),
            "tooling-router-clients ({mode:?})"
        );
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("HTTP routers do not have ordinary agent clients")
        );
    }
}

/// The grep tool extended with every surface a Go client spells differently:
/// an enum default, an optional stdin, a group with its own globals, a map
/// option, a defaulted positional, and one error name with two payloads.
fn go_grep_tool() -> Tool {
    let mut tool = grep_tool();
    let root = &mut tool.commands.nodes[0];
    root.globals.options[0].default = Some(SchemaValue::Enum { case: 2 });
    let body = root.body.as_mut().unwrap();
    body.stdin = Some(StreamSpec {
        doc: doc("haystack"),
        mime: vec![],
        required: false,
    });

    let replace = tool.commands.nodes[1].body.as_mut().unwrap();
    replace.stderr = Some(StreamSpec {
        doc: doc("diagnostics"),
        mime: vec![],
        required: false,
    });
    replace.errors = vec![ErrorCase {
        name: "bad-pattern".to_string(),
        doc: doc("bad pattern"),
        kind: ErrorKind::UsageError,
        exit_code: 2,
        payload: Some(SchemaType::u32()),
    }];

    let mut config = command_node("config");
    config.globals.options = vec![OptionSpec {
        default: Some(SchemaValue::String("dev".to_string())),
        ..option("profile", OptionShape::Scalar(SchemaType::string()))
    }];
    config.subcommands = vec![CommandIndex(3)];
    let mut get = command_node("get");
    get.body = Some(CommandBody {
        positionals: Positionals {
            fixed: vec![Positional {
                default: Some(SchemaValue::String("all".to_string())),
                required: false,
                ..positional("key", SchemaType::string())
            }],
            tail: None,
        },
        options: vec![option(
            "labels",
            OptionShape::RepeatableMap(RepeatableMapShape {
                repetition: Repetition::Repeated,
                map_type: SchemaType::map(SchemaType::string(), SchemaType::string()),
                duplicate_key_policy: DuplicateKeyPolicy::Reject,
            }),
        )],
        result: Some(ToolResultSpec {
            type_: SchemaType::option(SchemaType::string()),
            doc: doc("value"),
            formatters: vec![Formatter {
                name: "text".to_string(),
                doc: doc("text"),
            }],
            default_formatter: "text".to_string(),
        }),
        ..tool_body()
    });
    tool.commands.nodes[0].subcommands.push(CommandIndex(2));
    tool.commands.nodes.push(config);
    tool.commands.nodes.push(get);
    tool
}

/// A tool client declares the tool and its commands with the guest SDK's own
/// spec, so it must resolve without a definition error: the check runs a call,
/// which resolves the command before it finds there is no host to send to.
#[test]
fn go_guest_tool_client_is_gofmt_clean_vets_and_resolves(env: &GoEnv) {
    let generated = GeneratedGo::tool(env, go_grep_tool());
    let client = generated.read("client.go");
    for expected in [
        "type GrepTool struct{}",
        "var Tool = golem.DefineToolClient[GrepTool](\"grep\")",
        "var ErrIo = golem.DefineToolError[golem.Unit](Tool, \"io\"",
        "var ErrRootBadPattern = golem.DefineToolError[string](Tool, \"bad-pattern\"",
        "var ErrReplaceBadPattern = golem.DefineToolError[uint32](Tool, \"bad-pattern\"",
        "var _ = Tool.Globals[RootGlobals](",
        ".Default(ColorModeAuto)",
        "var configGroup = Tool.Group(\"config\")",
        "var _ = configGroup.Globals[ConfigGlobals](",
        "var Root = Tool.Body[RootArgs, []string](",
        "var Replace = Tool.OutputCommand[ReplaceArgs, golem.Unit](\"replace\"",
        "s.Stdout().Required()",
        "s.Stderr()\n",
        "var ConfigGet = configGroup.Command[ConfigGetArgs, values.Option[string]](\"get\"",
        "s.Positional(&a.Key).Default(\"all\")",
        "s.Map(&a.Labels)",
        "s.Stdin(&a.Stdin).Optional()",
        "s.CountFlag(&a.Verbosity)",
    ] {
        assert!(
            client.contains(expected),
            "missing {expected} in:\n{client}"
        );
    }
    assert!(!client.contains("var ErrBadPattern"), "{client}");
    generated.assert_gofmt_clean(env);
    generated.assert_vets_for_wasip1(env);
    let package = generated.package();
    generated.run_native_test(
        env,
        &format!(
            r#"package {package}

import (
	"strings"
	"testing"

	"github.com/golemcloud/golem/sdks/go/golem"
)

func TestTheToolClientResolves(t *testing.T) {{
	if errs := golem.DefinitionErrors(); len(errs) > 0 {{
		t.Fatalf("definition errors: %v", errs)
	}}
	calls := []error{{}}
	_, err := Root.Call(func(a *RootArgs) {{ a.Pattern = "x"; a.Files = []string{{"a"}} }})
	calls = append(calls, err)
	_, err = Replace.Call(func(a *ReplaceArgs) {{ a.Pattern = "x" }})
	calls = append(calls, err)
	_, err = ConfigGet.Call(func(a *ConfigGetArgs) {{ a.Labels = map[string]string{{"k": "v"}} }})
	calls = append(calls, err)
	for _, err := range calls[1:] {{
		if err == nil || !strings.Contains(err.Error(), "only available inside a component") {{
			t.Errorf("a call did not reach the host: %v", err)
		}}
	}}
}}
"#
        ),
    );
}

/// An ephemeral agent has no durable identity, so its guest client offers
/// phantoms only and declares the target's mode.
#[test]
fn go_guest_ephemeral_client_has_no_get(env: &GoEnv) {
    let request = agent(
        "RequestAgent",
        "rust",
        vec![field("route", SchemaType::string())],
        vec![method("run", vec![], Some(SchemaType::string()))],
        vec![],
        AgentMode::Ephemeral,
    );
    let generated = GeneratedGo::guest(env, request);
    let client = generated.read("client.go");
    assert!(
        client.contains("golem.AgentClientSpec{Mode: golem.Ephemeral}"),
        "{client}"
    );
    assert!(client.contains("func NewPhantomRequestAgent("), "{client}");
    assert!(!client.contains("func GetRequestAgent("), "{client}");
    generated.assert_gofmt_clean(env);
    generated.assert_vets_for_wasip1(env);
}

/// An ephemeral agent's external client constructs a phantom per call and
/// reports the instance that ran with each awaited result.
#[test]
fn go_external_ephemeral_client_reports_the_instance(env: &GoEnv) {
    let request = agent(
        "RequestAgent",
        "rust",
        vec![field("route", SchemaType::string())],
        vec![
            method("run", vec![], Some(SchemaType::string())),
            method("touch", vec![], None),
        ],
        vec![],
        AgentMode::Ephemeral,
    );
    let generated = GeneratedGo::external(env, request);
    let client = generated.read("client.go");
    for expected in [
        "func NewPhantomRequestAgent(",
        "Run(ctx context.Context) (bridge.InvocationResult[string], error)",
        "return bridge.CallWithID(ctx, c.agent, \"run\"",
        "Touch(ctx context.Context) (bridge.Receipt, error)",
    ] {
        assert!(
            client.contains(expected),
            "missing {expected} in:\n{client}"
        );
    }
    assert!(!client.contains("func GetRequestAgent("), "{client}");
    generated.assert_gofmt_clean(env);
    generated.assert_vets_natively(env);
}

fn configured_counter_agent() -> AgentTypeSchema {
    let mut configured = counter_agent();
    configured.schema.defs.push(def(
        "cfg.Db",
        SchemaType::record(vec![
            named_field("host", SchemaType::string()),
            named_field("mode", ref_to("cfg.Mode")),
        ]),
    ));
    configured.schema.defs.push(def(
        "cfg.Mode",
        SchemaType::r#enum(vec!["read-only".into(), "read-write".into()]),
    ));
    configured.config = vec![
        local_config(vec!["greeting"], SchemaType::string()),
        local_config(vec!["limits", "max-items"], SchemaType::u16()),
        local_config(vec!["db"], ref_to("cfg.Db")),
        AgentConfigDeclarationSchema {
            source: AgentConfigSource::Secret,
            path: vec!["token".into()],
            value_type: SchemaType::string(),
        },
    ];
    configured
}

/// A guest client of an agent with local configuration gets the same typed
/// configuration struct, applied as per-path entries.
#[test]
fn go_guest_client_overrides_typed_configuration(env: &GoEnv) {
    let generated = GeneratedGo::guest(env, configured_counter_agent());
    let client = generated.read("client.go");
    for expected in [
        "type CounterAgentConfig struct",
        "LimitsMaxItems values.Option[uint16]",
        "func WithCounterAgentConfig(cfg CounterAgentConfig) golem.ClientOpt {",
        "golem.ConfigEntryOf([]string{\"limits\", \"max-items\"}, v)",
        "func GetCounterAgent(id CounterAgentId, opts ...golem.ClientOpt) CounterAgentClient {",
    ] {
        assert!(
            client.contains(expected),
            "missing {expected} in:\n{client}"
        );
    }
    assert!(!client.contains("Token"), "{client}");
    generated.assert_gofmt_clean(env);
    generated.assert_vets_for_wasip1(env);
}

/// An agent with local configuration gets a typed configuration struct whose
/// set fields travel as canonical JSON; secret configuration is left out.
#[test]
fn go_external_client_overrides_typed_configuration(env: &GoEnv) {
    let generated = GeneratedGo::external(env, configured_counter_agent());
    let client = generated.read("client.go");
    assert!(
        client.contains("type CounterAgentConfig struct"),
        "{client}"
    );
    assert!(!client.contains("Token"), "{client}");
    generated.assert_gofmt_clean(env);
    generated.assert_vets_natively(env);

    let package = generated.package();
    generated.run_native_test(
        env,
        &format!(
            r#"package {package}

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/golemcloud/golem/sdks/go/bridge"
	"github.com/golemcloud/golem/sdks/go/core/values"
)

func TestSetConfigurationFieldsTravelAsCanonicalJSON(t *testing.T) {{
	var config string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {{
		raw, _ := io.ReadAll(r.Body)
		var body map[string]json.RawMessage
		_ = json.Unmarshal(raw, &body)
		config = string(body["config"])
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{{"agentId":{{"componentId":"c","agentId":"a"}},"idempotencyKey":"k"}}`)
	}}))
	defer server.Close()

	client, err := GetCounterAgent(CounterAgentId{{Name: "c1"}},
		bridge.WithConfiguration(bridge.Configuration{{
			Server: bridge.Custom(server.URL, "token"), AppName: "app", EnvName: "env",
		}}),
		WithCounterAgentConfig(CounterAgentConfig{{
			LimitsMaxItems: values.Some[uint16](7),
			Db:             values.Some(CfgDb{{Host: "h", Mode: CfgModeReadWrite}}),
		}}),
	)
	if err != nil {{
		t.Fatal(err)
	}}
	if err := client.Add(context.Background(), 1); err != nil {{
		t.Fatal(err)
	}}
	want := `[{{"path":["limits","max-items"],"value":7}},{{"path":["db"],"value":{{"host":"h","mode":"read-write"}}}}]`
	if config != want {{
		t.Fatalf("config sent as %s, want %s", config, want)
	}}
}}
"#
        ),
    );
}

fn restricted_agent() -> AgentTypeSchema {
    let bounded = SchemaType::U32 {
        restrictions: Some(NumericRestrictions {
            min: Some(NumericBound::Unsigned(1)),
            max: Some(NumericBound::Unsigned(10)),
            unit: Some("px".into()),
        }),
        metadata: Default::default(),
    };
    let label = SchemaType::text(TextRestrictions {
        languages: Some(vec!["en".into(), "de".into()]),
        min_length: None,
        max_length: Some(40),
        regex: Some("^[a-z]+,[0-9]$".into()),
    });
    let output = SchemaType::path(PathSpec {
        direction: PathDirection::Output,
        kind: PathKind::File,
        allowed_mime_types: None,
        allowed_extensions: Some(vec!["png".into()]),
    });
    agent(
        "ShapeAgent",
        "rust",
        vec![field("name", SchemaType::string())],
        vec![method(
            "draw",
            vec![
                field("box", ref_to("shape.Box")),
                field("count", SchemaType::option(bounded.clone())),
                field("out", output),
            ],
            None,
        )],
        vec![def(
            "shape.Box",
            SchemaType::record(vec![
                named_field("width", bounded),
                named_field("name", SchemaType::string()),
                named_field("label", label.clone()),
                named_field("tags", SchemaType::list(label)),
            ]),
        )],
        AgentMode::Durable,
    )
}

/// A restricted field carries a `golem` tag, so the schema the SDK derives
/// from the generated type matches the one it came from; the tags line up as
/// gofmt writes them.
#[test]
fn go_clients_tag_restricted_fields(env: &GoEnv) {
    for generated in [
        GeneratedGo::guest(env, restricted_agent()),
        GeneratedGo::external(env, restricted_agent()),
    ] {
        let types = generated.read("types.go");
        for expected in [
            r#"`golem:"min=1,max=10,unit=px"`"#,
            r#"`golem:"languages=en|de,maxLength=40,regex=^[a-z]+,[0-9]$"`"#,
        ] {
            assert!(types.contains(expected), "missing {expected} in:\n{types}");
        }
        generated.assert_gofmt_clean(env);
    }
    let guest = GeneratedGo::guest(env, restricted_agent());
    let client = guest.read("client.go");
    for expected in [
        r#"`golem:"min=1,max=10,unit=px"`"#,
        r#"`golem:"direction=output,kind=file,extensions=png"`"#,
    ] {
        assert!(
            client.contains(expected),
            "missing {expected} in:\n{client}"
        );
    }
    guest.assert_vets_for_wasip1(env);
}

fn unstructured(role: Role, inline: SchemaType) -> SchemaType {
    let mut typ = SchemaType::variant(vec![
        variant_case("inline", Some(inline)),
        variant_case("url", Some(SchemaType::url(Default::default()))),
    ]);
    typ.metadata_mut().role = Some(role);
    typ
}

fn content_agent() -> AgentTypeSchema {
    let any_text = || unstructured(Role::UnstructuredText, SchemaType::text(Default::default()));
    let any_binary = || {
        unstructured(
            Role::UnstructuredBinary,
            SchemaType::binary(Default::default()),
        )
    };
    let doc = unstructured(
        Role::UnstructuredText,
        SchemaType::text(TextRestrictions {
            languages: Some(vec!["en".into(), "de".into()]),
            min_length: None,
            max_length: None,
            regex: None,
        }),
    );
    let basic = multimodal(vec![
        variant_case("Text", Some(any_text())),
        variant_case("Binary", Some(any_binary())),
    ]);
    let figures = multimodal(vec![
        variant_case("Caption", Some(any_text())),
        variant_case("Chart", Some(SchemaType::list(SchemaType::s32()))),
    ]);
    agent(
        "ContentAgent",
        "rust",
        vec![field("name", SchemaType::string())],
        vec![
            method(
                "summarize",
                vec![field("doc", doc), field("photo", any_binary())],
                Some(basic),
            ),
            method("figures", vec![], Some(figures)),
        ],
        vec![],
        AgentMode::Durable,
    )
}

/// Role-marked content is spelled with the SDK's shared content types, not
/// declared as variants, with a generated marker for a language list.
#[test]
fn go_clients_spell_unstructured_and_multimodal_content(env: &GoEnv) {
    for generated in [
        GeneratedGo::guest(env, content_agent()),
        GeneratedGo::external(env, content_agent()),
    ] {
        let client = generated.read("client.go");
        for expected in [
            "values.UnstructuredText[LanguagesEnDe]",
            "values.UnstructuredBinary[values.AnyMimeType]",
            "values.Multimodal",
            "values.MultimodalOf[",
        ] {
            assert!(
                client.contains(expected),
                "missing {expected} in:\n{client}"
            );
        }
        let types = generated.read("types.go");
        assert!(types.contains("type LanguagesEnDe struct{}"), "{types}");
        assert!(types.contains("Caption"), "{types}");
        assert!(
            !types.contains("Inline"),
            "an unstructured variant was declared:\n{types}"
        );
        generated.assert_gofmt_clean(env);
    }
    let guest = GeneratedGo::guest(env, content_agent());
    guest.assert_vets_for_wasip1(env);
    let external = GeneratedGo::external(env, content_agent());
    external.assert_vets_natively(env);
    let code = external.read("client.go") + &external.read("codec.go");
    for expected in [
        "bridge.EncodeUnstructuredText[LanguagesEnDe]",
        "bridge.DecodeMultimodal(sv, bridge.DecodeModality)",
    ] {
        assert!(code.contains(expected), "missing {expected} in:\n{code}");
    }
}
