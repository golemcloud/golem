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

func cfgAgent(d *definitions) *Agent[cfgId, cfgState] {
	return defineAgentInto[cfgId, cfgState](
		d,
		Spec{Name: "Cfg"},
		func(cfgId) *cfgState { return &cfgState{} },
	)
}

// TestConfigDeclarationsInMetadata — A declared local config and a secret both land in the agent-type metadata, the
// secret carrying a secret(inner) value type.
func TestConfigDeclarationsInMetadata(t *testing.T) {
	withDefs(t, func(d *definitions) {
		a := cfgAgent(d)
		defineConfigInto[string](d, a, []string{"greeting"})
		defineSecretInto[string](d, a, []string{"api", "key"})

		out, errs := d.discover()
		if len(errs) != 0 {
			t.Fatalf("unexpected definition errors: %v", errs)
		}
		at := out[0]
		if len(at.Config) != 2 {
			t.Fatalf("want 2 config declarations, got %d", len(at.Config))
		}

		var local, secret *common.AgentConfigDeclaration
		for i := range at.Config {
			switch at.Config[i].Source {
			case common.AgentConfigSourceLocal:
				local = &at.Config[i]
			case common.AgentConfigSourceSecret:
				secret = &at.Config[i]
			}
		}

		if local == nil || !pathsEqual(local.Path, []string{"greeting"}) {
			t.Fatalf("local declaration wrong: %+v", local)
		}
		if body := at.Schema.TypeNodes[local.ValueType].Body; body.Tag() != types.SchemaTypeBodyStringType {
			t.Errorf("local value type tag = %d, want string(%d)", body.Tag(), types.SchemaTypeBodyStringType)
		}

		if secret == nil || !pathsEqual(secret.Path, []string{"api", "key"}) {
			t.Fatalf("secret declaration wrong: %+v", secret)
		}
		if body := at.Schema.TypeNodes[secret.ValueType].Body; body.Tag() != types.SchemaTypeBodySecretType {
			t.Errorf("secret value type tag = %d, want secret(%d)", body.Tag(), types.SchemaTypeBodySecretType)
		}
	})
}

// TestMisuseDuplicateConfigPath — The same path must not be declared twice on one agent, even across a plain
// config and a secret.
func TestMisuseDuplicateConfigPath(t *testing.T) {
	withDefs(t, func(d *definitions) {
		a := cfgAgent(d)
		defineConfigInto[string](d, a, []string{"db", "url"})
		defineSecretInto[string](d, a, []string{"db", "url"})
		mustDefErr(t, d, "declared more than once")
	})
}

func TestMisuseConfigEmptyPath(t *testing.T) {
	withDefs(t, func(d *definitions) {
		a := cfgAgent(d)
		defineConfigInto[string](d, a, nil)
		mustDefErr(t, d, "empty path")
	})
}

func TestMisuseConfigEmptySegment(t *testing.T) {
	withDefs(t, func(d *definitions) {
		a := cfgAgent(d)
		defineSecretInto[string](d, a, []string{"db", ""})
		mustDefErr(t, d, "empty segment")
	})
}

func TestMisuseConfigUnknownAgent(t *testing.T) {
	withDefs(t, func(d *definitions) {
		// An agent value that was never registered in d.
		ghost := &Agent[cfgId, cfgState]{name: "Ghost"}
		defineConfigInto[string](d, ghost, []string{"k"})
		mustDefErr(t, d, "unknown agent")
	})
	withDefs(t, func(d *definitions) {
		var nilAgent *Agent[cfgId, cfgState]
		defineConfigInto[string](d, nilAgent, []string{"k"})
		mustDefErr(t, d, "nil agent")
	})
}

// TestConfigDecodeLocalValue — decodeConfigValue decodes a get-config-value result tree through the value
// type's codec — the pure half of a local read.
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

// TestExtractSecretHandle — extractSecretHandle pulls the secret handle out of a get-config-value result,
// and rejects a tree that is not a secret value — the pure half of a secret read.
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

// TestConfigOverrideEncodesAndValidates — A config override encodes the value and, validated against the target's
// declarations, becomes a typed-agent-config-value threaded into make-wasm-rpc.
func TestConfigOverrideEncodesAndValidates(t *testing.T) {
	withDefs(t, func(d *definitions) {
		a := cfgAgent(d)
		key := defineConfigInto[string](d, a, []string{"db", "url"})
		noDefErrs(t, d)

		var o clientOpts
		WithConfigValue(key, "prod-url")(&o)

		got, err := buildAgentConfig(d, d.agents["Cfg"], o.configs)
		if err != nil {
			t.Fatalf("buildAgentConfig: %v", err)
		}
		if len(got) != 1 {
			t.Fatalf("want 1 override, got %d", len(got))
		}
		if !pathsEqual(got[0].Path, []string{"db", "url"}) {
			t.Fatalf("override path = %v", got[0].Path)
		}
		val, err := decodeConfigValue[string](d, got[0].Path, got[0].Value.Value)
		if err != nil {
			t.Fatalf("decode override value: %v", err)
		}
		if val != "prod-url" {
			t.Fatalf("override value = %q, want %q", val, "prod-url")
		}
	})
}

// TestConfigOverrideUndeclaredRejected — Overriding a key the target agent does not declare is rejected client-side.
func TestConfigOverrideUndeclaredRejected(t *testing.T) {
	withDefs(t, func(d *definitions) {
		a := cfgAgent(d)
		defineConfigInto[string](d, a, []string{"declared"})

		undeclared := &Config[string]{path: []string{"not", "declared"}}
		var o clientOpts
		WithConfigValue(undeclared, "x")(&o)
		if _, err := buildAgentConfig(d, d.agents["Cfg"], o.configs); err == nil {
			t.Fatal("expected an error overriding an undeclared config key")
		}
	})
}

func TestConfigOverrideNilKey(t *testing.T) {
	withDefs(t, func(d *definitions) {
		cfgAgent(d)
		var o clientOpts
		WithConfigValue[string](nil, "x")(&o)
		if _, err := buildAgentConfig(d, d.agents["Cfg"], o.configs); err == nil {
			t.Fatal("expected an error for a nil config key")
		}
	})
}

// --- struct/record authoring (ConfigOf / LoadConfig) ---

type demoDBConfig struct {
	Url      string
	Password Secret[string]
}
type demoAppConfig struct {
	Greeting string
	Db       demoDBConfig
}

// TestConfigOfFlattensStruct — ConfigOf flattens a struct — nested structs into multi-segment paths, Secret
// fields into secret leaves — producing the same declarations as per-key defines.
func TestConfigOfFlattensStruct(t *testing.T) {
	withDefs(t, func(d *definitions) {
		defineAgentInto[cfgId, cfgState](
			d,
			Spec{Name: "Cfg", Config: ConfigOf[demoAppConfig]()},
			func(cfgId) *cfgState { return &cfgState{} },
		)
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
			t.Fatalf("want %d declarations, got %d: %+v", len(want), len(at.Config), at.Config)
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
			if src == common.AgentConfigSourceSecret {
				if body := at.Schema.TypeNodes[dcl.ValueType].Body; body.Tag() != types.SchemaTypeBodySecretType {
					t.Errorf("%s: value type tag = %d, want secret", key, body.Tag())
				}
			}
		}
	})
}

func TestConfigOfNonStructReported(t *testing.T) {
	withDefs(t, func(d *definitions) {
		defineAgentInto[cfgId, cfgState](
			d,
			Spec{Name: "Cfg", Config: ConfigOf[int]()},
			func(cfgId) *cfgState { return &cfgState{} },
		)
		mustDefErr(t, d, "requires a struct")
	})
}

// TestMaterializeConfig — materializeConfig assembles the struct from per-leaf reads — the pure half of
// LoadConfig, tested with a fake reader (the real reader hits the host).
func TestMaterializeConfig(t *testing.T) {
	got, err := materializeConfig[demoAppConfig](func(lf configLeaf) (reflect.Value, error) {
		switch strings.Join(lf.path, "/") {
		case "greeting":
			return reflect.ValueOf("hello"), nil
		case "db/url":
			return reflect.ValueOf("db://x"), nil
		case "db/password":
			return reflect.ValueOf(NewSecret("s3cr3t")), nil
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
	if got.Db.Password.Reveal() != "s3cr3t" {
		t.Errorf("Db.Password = %q", got.Db.Password.Reveal())
	}
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
