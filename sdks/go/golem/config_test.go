// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package golem

import (
	"fmt"
	"reflect"
	"strings"
	"testing"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	secrets "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_secrets_types"
)

// Config/secret declaration and the pure read core are unit-tested here; the
// actual get-config-value/reveal host calls are wasm-only and covered by the
// integration layers (see the go-sdk-testing-strategy notes).

type cfgId struct{ Name string }
type cfgState struct{}

// demoDBConfig / demoAppConfig is a config struct with a nested struct and a
// secret field, exercising flattening at every position.
type demoDBConfig struct {
	Url      string
	Password Secret[string]
}
type demoAppConfig struct {
	Greeting string
	Db       demoDBConfig
}

// cfgConfiguredAgent registers the "Cfg" agent with config type Cfg attached (a
// no-op constructor — the host read must stay out of the natively-linked test).
func cfgConfiguredAgent[Cfg any](d *definitions) *Agent[cfgId, cfgState, Cfg] {
	return defineConfiguredAgentInto[cfgId, cfgState, Cfg](
		d,
		Spec{Name: "Cfg"},
		func(*InitContext[cfgId, cfgState, Cfg]) *cfgState { return &cfgState{} },
	)
}

// TestConfigDeclarationsInMetadata — DefineConfiguredAgent flattens the agent's
// config struct into the agent-type metadata: local fields as local declarations,
// the secret field carrying a secret(inner) value type.
func TestConfigDeclarationsInMetadata(t *testing.T) {
	withDefs(t, func(d *definitions) {
		cfgConfiguredAgent[demoAppConfig](d)

		out, errs := d.discover()
		if len(errs) != 0 {
			t.Fatalf("unexpected definition errors: %v", errs)
		}
		at := out[0]

		want := map[string]common.AgentConfigSource{
			"greeting":    common.AgentConfigSourceLocal,
			"db/url":      common.AgentConfigSourceLocal,
			"db/password": common.AgentConfigSourceSecret,
		}
		if len(at.Config) != len(want) {
			t.Fatalf("want %d config declarations, got %d: %+v", len(want), len(at.Config), at.Config)
		}
		for _, dcl := range at.Config {
			key := strings.Join(dcl.Path, "/")
			src, ok := want[key]
			if !ok {
				t.Errorf("unexpected config path %q", key)
				continue
			}
			if dcl.Source != src {
				t.Errorf("%s: source = %d, want %d", key, dcl.Source, src)
			}
			body := at.Schema.TypeNodes[dcl.ValueType].Body
			if src == common.AgentConfigSourceSecret && body.Tag() != types.SchemaTypeBodySecretType {
				t.Errorf("%s: value type tag = %d, want secret(%d)", key, body.Tag(), types.SchemaTypeBodySecretType)
			}
			if key == "greeting" && body.Tag() != types.SchemaTypeBodyStringType {
				t.Errorf("greeting: value type tag = %d, want string(%d)", body.Tag(), types.SchemaTypeBodyStringType)
			}
		}
	})
}

func TestConfigNonStructReported(t *testing.T) {
	withDefs(t, func(d *definitions) {
		cfgConfiguredAgent[int](d)
		mustDefErr(t, d, "must be a struct")
	})
}

// TestMisuseDuplicateConfigPath — the same path must not be recorded twice on one
// agent. (Not reachable from a single config struct — field names are unique — so
// the defensive check in recordConfigOn is exercised directly.)
func TestMisuseDuplicateConfigPath(t *testing.T) {
	withDefs(t, func(d *definitions) {
		cfgConfiguredAgent[NoConfig](d)
		e := d.agents["Cfg"]
		recordConfigOn(d, e, "Cfg", common.AgentConfigSourceLocal, []string{"k"}, reflect.TypeFor[string]())
		recordConfigOn(d, e, "Cfg", common.AgentConfigSourceLocal, []string{"k"}, reflect.TypeFor[string]())
		mustDefErr(t, d, "declared more than once")
	})
}

// TestConfigDecodeLocalValue — decodeConfigValue decodes a get-config-value result
// tree through the value type's codec — the pure half of a local read.
func TestConfigDecodeLocalValue(t *testing.T) {
	d := newDefinitions()
	tree := types.SchemaValueTree{
		ValueNodes: []types.SchemaValueNode{types.MakeSchemaValueNodeStringValue("hi")},
		Root:       0,
	}
	got, err := decodeConfigValue[string](d, []string{"greeting"}, tree)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	if got != "hi" {
		t.Fatalf("decoded %q, want %q", got, "hi")
	}
}

// TestExtractSecretHandle — extractSecretHandle pulls the secret handle out of a
// get-config-value result, and rejects a tree that is not a secret value — the
// pure half of a secret read.
func TestExtractSecretHandle(t *testing.T) {
	// A real *Secret can only be built by the host (its constructor pulls in a
	// wasmimport), so use a nil handle: this still exercises the tag dispatch and
	// that the handle at the tree root is returned. The revealed round trip is a
	// host concern, covered by integration.
	var handle *types.Secret
	tree := types.SchemaValueTree{
		ValueNodes: []types.SchemaValueNode{types.MakeSchemaValueNodeSecretValue(handle)},
		Root:       0,
	}
	got, err := extractSecretHandle([]string{"api"}, tree)
	if err != nil {
		t.Fatalf("extract: %v", err)
	}
	if got != handle {
		t.Fatalf("extracted a different handle")
	}

	notSecret := types.SchemaValueTree{
		ValueNodes: []types.SchemaValueNode{types.MakeSchemaValueNodeStringValue("nope")},
		Root:       0,
	}
	if _, err := extractSecretHandle([]string{"api"}, notSecret); err == nil {
		t.Fatal("expected an error extracting a non-secret value")
	}
}

// TestMaterializeConfig — materializeConfig assembles the struct from per-leaf
// reads — the pure half of AgentConfig.Get, tested with a fake reader (the real
// reader hits the host).
func TestMaterializeConfig(t *testing.T) {
	got, err := materializeConfig[demoAppConfig](func(lf configLeaf) (reflect.Value, error) {
		switch strings.Join(lf.path, "/") {
		case "greeting":
			return reflect.ValueOf("hello"), nil
		case "db/url":
			return reflect.ValueOf("db://x"), nil
		case "db/password":
			// A config secret is a handle backed by a live-read closure, not a value.
			return reflect.ValueOf(Secret[string]{read: func() (string, error) { return "s3cr3t", nil }}), nil
		default:
			return reflect.Value{}, fmt.Errorf("unexpected leaf %v", lf.path)
		}
	})
	if err != nil {
		t.Fatalf("materialize: %v", err)
	}
	if got.Greeting != "hello" {
		t.Errorf("Greeting = %q", got.Greeting)
	}
	if got.Db.Url != "db://x" {
		t.Errorf("Db.Url = %q", got.Db.Url)
	}
	if pw := got.Db.Password.Get(); pw != "s3cr3t" {
		t.Errorf("Db.Password.Get() = %q", pw)
	}
}

// TestSecretGetIsLive — Secret.Get re-reads on every call (so a rotated secret is
// observed, not a cached snapshot), and a zero-value Secret panics with a clear
// message rather than a cryptic nil-closure dereference.
func TestSecretGetIsLive(t *testing.T) {
	n := 0
	s := Secret[string]{read: func() (string, error) { n++; return fmt.Sprintf("v%d", n), nil }}
	if a := s.Get(); a != "v1" {
		t.Fatalf("first Get = %q, want v1", a)
	}
	if b := s.Get(); b != "v2" {
		t.Fatalf("second Get = %q, want v2 (Get must re-read, not cache)", b)
	}
	func() {
		defer func() {
			if recover() == nil {
				t.Fatal("zero-value Secret.Get should panic")
			}
		}()
		var zero Secret[string]
		_ = zero.Get()
	}()
}

// TestWithConfigEncodesLocalsSkipsSecrets — WithConfig encodes each local leaf of
// the config value as an override (validated against the target's declarations)
// and skips the platform-provisioned secret leaf.
func TestWithConfigEncodesLocalsSkipsSecrets(t *testing.T) {
	withDefs(t, func(d *definitions) {
		cfgConfiguredAgent[demoAppConfig](d)
		noDefErrs(t, d)

		var o clientOpts
		WithConfig(demoAppConfig{
			Greeting: "hi",
			Db:       demoDBConfig{Url: "db://prod"},
		})(&o)

		got, err := buildAgentConfig(d, d.agents["Cfg"], o.configs)
		if err != nil {
			t.Fatalf("buildAgentConfig: %v", err)
		}
		// greeting + db/url; db/password (secret) is skipped.
		if len(got) != 2 {
			t.Fatalf("want 2 overrides, got %d: %+v", len(got), got)
		}
		byPath := map[string]types.TypedSchemaValue{}
		for _, tv := range got {
			byPath[strings.Join(tv.Path, "/")] = tv.Value
		}
		if _, ok := byPath["db/password"]; ok {
			t.Error("secret leaf should not be sent as an override")
		}
		val, err := decodeConfigValue[string](d, []string{"greeting"}, byPath["greeting"].Value)
		if err != nil {
			t.Fatalf("decode greeting override: %v", err)
		}
		if val != "hi" {
			t.Fatalf("greeting override = %q, want %q", val, "hi")
		}
	})
}

// TestWithConfigUndeclaredRejected — Overriding config against an agent that
// declares none is rejected client-side (no key matches). Encodes demoAppConfig
// overrides directly and validates them against a no-config agent's entry.
func TestWithConfigUndeclaredRejected(t *testing.T) {
	withDefs(t, func(d *definitions) {
		cfgConfiguredAgent[NoConfig](d) // agent "Cfg" declares no config
		var o clientOpts
		o.configs = append(o.configs, func(dd *definitions) ([]common.TypedAgentConfigValue, error) {
			return encodeConfigOverrides(dd, demoAppConfig{Greeting: "x"})
		})
		if _, err := buildAgentConfig(d, d.agents["Cfg"], o.configs); err == nil {
			t.Fatal("expected an error overriding an undeclared config key")
		}
	})
}

func TestSecretErrorToGo(t *testing.T) {
	cases := []struct {
		name string
		in   secrets.SecretError
		want string
	}{
		{"unavailable", secrets.MakeSecretErrorUnavailable("source down"), "unavailable"},
		{"internal", secrets.MakeSecretErrorInternal("boom"), "internal error"},
	}
	for _, c := range cases {
		got := secretErrorToGo([]string{"api", "key"}, c.in).Error()
		if !strings.Contains(got, c.want) {
			t.Errorf("%s: %q does not contain %q", c.name, got, c.want)
		}
		if !strings.Contains(got, "api") {
			t.Errorf("%s: error should name the path: %q", c.name, got)
		}
	}
}
