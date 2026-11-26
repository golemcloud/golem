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
	"encoding/json"
	"fmt"
	"os"
	"reflect"
	"strings"
	"testing"

	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"github.com/golemcloud/golem/sdks/go/golem/internal/witschema"
)

type staticSettings struct {
	Label string
	Count uint32
}

type staticChoice interface{ staticChoice() }
type staticText struct{ Value string }
type staticOther struct{ Value string }

func (staticText) staticChoice()  {}
func (staticOther) staticChoice() {}

type staticNode struct {
	DisplayName string
	Next        *staticNode
}

type staticReference struct {
	Settings staticSettings                 `golemDefault:"{\"label\":\"code-default\",\"count\":31}"`
	Choice   staticChoice                   `golemDefault:"\"selected:value\""`
	Slots    Tuple3[*int32, *int32, string] `golemDefault:"[null,7,\"retained\"]"`
	Omitted  *string
}

// Consume the hand-authored corpus, not an expectation generated from Go's
// codecs. Host layer resolution is tested by the host; here its resolved values
// cross the canonical/WIT/native boundary that the guest reads.
func TestStaticConfigCorpus(t *testing.T) {
	raw, err := os.ReadFile("../../../test-data/config-conformance/v1.json")
	if err != nil {
		t.Fatal(err)
	}
	var corpus struct {
		Schemas map[string]struct {
			Declarations []struct {
				Path         []string
				DefaultValue *string `json:"default_value"`
			}
		}
		StaticCases []struct {
			ID       string
			Schema   string
			Expected map[string]json.RawMessage
			Error    string
		}
	}
	if err := json.Unmarshal(raw, &corpus); err != nil {
		t.Fatal(err)
	}
	withDefs(t, func(d *definitions) {
		defineUnionInto[staticChoice](d,
			WrappedBranch[staticText]("Text", ByPrefix("selected:")),
			WrappedBranch[staticOther]("Other", ByPrefix("other:")),
		)
		cfgConfiguredAgent[staticReference](d)
		out, errs := d.discover()
		if len(errs) != 0 {
			t.Fatal(errs)
		}
		at := out[0]
		graph, err := witschema.GraphToCore(at.Schema)
		if err != nil {
			t.Fatal(err)
		}
		want := corpus.Schemas["reference"].Declarations
		if len(at.Config) != len(want) {
			t.Fatalf("declarations = %d, want %d", len(at.Config), len(want))
		}
		for i, decl := range at.Config {
			if !reflect.DeepEqual(decl.Path, want[i].Path) {
				t.Fatalf("path = %v, want %v", decl.Path, want[i].Path)
			}
			if decl.DefaultValue.IsSome() != (want[i].DefaultValue != nil) {
				t.Fatalf("%v: default presence differs", decl.Path)
			}
			if want[i].DefaultValue != nil {
				value, err := witschema.ValueToCore(decl.DefaultValue.Some())
				if err != nil {
					t.Fatal(err)
				}
				ref, err := graph.Ref(decl.ValueType)
				if err != nil {
					t.Fatal(err)
				}
				json, err := ref.UnpackJSONBytes(value)
				if err != nil {
					t.Fatal(err)
				}
				assertConfigJSON(t, json, []byte(*want[i].DefaultValue))
			}
		}
		for _, tc := range corpus.StaticCases {
			if tc.Schema != "reference" || tc.Error != "" {
				continue
			}
			t.Run(tc.ID, func(t *testing.T) {
				got, err := materializeConfig[staticReference](func(lf configLeaf) (reflect.Value, error) {
					for _, decl := range at.Config {
						if !pathsEqual(decl.Path, lf.path) {
							continue
						}
						ref, err := graph.Ref(decl.ValueType)
						if err != nil {
							return reflect.Value{}, err
						}
						v, err := ref.PackJSONBytes(tc.Expected[lf.path[0]])
						if err != nil {
							return reflect.Value{}, err
						}
						tree, err := witschema.ValueToWit(v)
						if err != nil {
							return reflect.Value{}, err
						}
						dst := reflect.New(lf.typ).Elem()
						dec := engine.Decoder{Nodes: tree.ValueNodes}
						err = d.Compile(lf.typ).Decode(&dec, dst, tree.Root)
						return dst, err
					}
					return reflect.Value{}, fmt.Errorf("missing declaration %v", lf.path)
				})
				if err != nil {
					t.Fatal(err)
				}
				// Assert native union identity and optional positions independently
				// before re-encoding through the ordinary creation-override path.
				if tc.ID == "static/positional-absence-and-branch" {
					if got.Choice != (staticOther{"other:changed"}) || got.Slots.A == nil || *got.Slots.A != 11 || got.Slots.B != nil || got.Slots.C != "third" {
						t.Fatalf("branch or tuple positions lost: choice %T %+v, slots %+v", got.Choice, got.Choice, got.Slots)
					}
				} else if got.Choice != (staticText{"selected:value"}) || got.Slots.A != nil || got.Slots.B == nil || *got.Slots.B != 7 || got.Slots.C != "retained" {
					t.Fatalf("default branch or tuple positions lost: %+v", got)
				}
				entries, err := encodeConfigOverrides(d, got)
				if err != nil {
					t.Fatal(err)
				}
				for i, entry := range entries {
					ref, _ := graph.Ref(at.Config[i].ValueType)
					value, err := witschema.ValueToCore(entry.Value.Value)
					if err != nil {
						t.Fatal(err)
					}
					encoded, err := ref.UnpackJSONBytes(value)
					if err != nil {
						t.Fatal(err)
					}
					assertConfigJSON(t, encoded, tc.Expected[entry.Path[0]])
				}
			})
		}
	})
}

func assertConfigJSON(t *testing.T, actual, expected []byte) {
	t.Helper()
	var a, b any
	if err := json.Unmarshal(actual, &a); err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(expected, &b); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(a, b) {
		t.Fatalf("JSON = %s, want %s", actual, expected)
	}
}

func TestStaticConfigRecursiveDefault(t *testing.T) {
	type config struct {
		Tree     staticNode `golemDefault:"{\"displayName\":\"outer\",\"next\":{\"displayName\":\"inner\",\"next\":null}}"`
		APIKey   string     `golemDefault:"\"public-label\""`
		MaxChars uint32     `golemDefault:"19"`
	}
	withDefs(t, func(d *definitions) {
		cfgConfiguredAgent[config](d)
		out, errs := d.discover()
		if len(errs) != 0 {
			t.Fatal(errs)
		}
		at := out[0]
		if len(at.Config) != 3 || !reflect.DeepEqual(at.Config[0].Path, []string{"tree"}) ||
			!reflect.DeepEqual(at.Config[1].Path, []string{"apiKey"}) || !reflect.DeepEqual(at.Config[2].Path, []string{"maxChars"}) {
			t.Fatalf("records or canonical names changed: %+v", at.Config)
		}
		tree := at.Config[0].DefaultValue.Some()
		got, err := decodeConfigValue[staticNode](d, []string{"tree"}, tree)
		if err != nil {
			t.Fatal(err)
		}
		want := staticNode{"outer", &staticNode{"inner", nil}}
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("recursive value = %+v, want %+v", got, want)
		}
	})
}

func TestStaticConfigDoesNotFallbackAtRead(t *testing.T) {
	type config struct {
		Enabled  bool     `golemDefault:"true"`
		Names    []string `golemDefault:"[\"from-code\"]"`
		Label    string   `golemDefault:"\"from-code\""`
		Optional *string  `golemDefault:"\"from-code\""`
	}
	got, err := materializeConfig[config](func(lf configLeaf) (reflect.Value, error) {
		return reflect.Zero(lf.typ), nil
	})
	if err != nil || got.Enabled || len(got.Names) != 0 || got.Label != "" || got.Optional != nil {
		t.Fatalf("host's explicit empty values were replaced: %+v, %v", got, err)
	}
	_, err = materializeConfig[config](func(lf configLeaf) (reflect.Value, error) {
		return reflect.Value{}, fmt.Errorf("host rejected supplied value")
	})
	if err == nil || !strings.Contains(err.Error(), "host rejected supplied value") {
		t.Fatalf("host error replaced by code default: %v", err)
	}
}

func TestStaticConfigRejectsInvalidDeclarations(t *testing.T) {
	check := func(t *testing.T, register func(*definitions), want string) {
		t.Helper()
		withDefs(t, func(d *definitions) {
			register(d)
			_, errs := d.discover()
			if !strings.Contains(fmt.Sprint(errs), want) {
				t.Fatalf("errors = %v, want %q", errs, want)
			}
		})
	}
	t.Run("wrong type", func(t *testing.T) {
		check(t, func(d *definitions) {
			cfgConfiguredAgent[struct {
				Count uint32 `golemDefault:"\"not-a-number\""`
			}](d)
		}, "invalid code default")
	})
	t.Run("restriction", func(t *testing.T) {
		check(t, func(d *definitions) {
			cfgConfiguredAgent[struct {
				Count uint32 `golem:"max=10" golemDefault:"11"`
			}](d)
		}, "invalid code default")
	})
	t.Run("secret default", func(t *testing.T) {
		check(t, func(d *definitions) {
			cfgConfiguredAgent[struct {
				Token Secret[string] `golemDefault:"\"secret\""`
			}](d)
		}, "cannot declare code defaults")
	})
	t.Run("nested secret", func(t *testing.T) {
		check(t, func(d *definitions) {
			cfgConfiguredAgent[struct{ Payload *[]Secret[string] }](d)
		}, "static data cannot contain Secret")
	})
}
