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

use crate::bridge_gen::fixtures::{agent, def, field, method, named_field, ref_to, variant_case};
use camino::{Utf8Path, Utf8PathBuf};
use golem_cli::app::build::go_toolchain::{GoToolchain, ensure_go_toolchain};
use golem_cli::bridge_gen::BridgeGenerator;
use golem_cli::bridge_gen::go::{GoBridgeGenerator, GoBridgeMode};
use golem_cli::model::app::ApplicationConfig;
use golem_cli::sdk_overrides::workspace_root;
use golem_common::model::agent::AgentMode;
use golem_common::schema::schema_type::{DiscriminatorRule, ResultSpec, UnionBranch, UnionSpec};
use golem_common::schema::{AgentTypeSchema, SchemaType};
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
        client.contains("golem.DeclareRemoteAgent[CounterAgentId](\"CounterAgent\")"),
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
