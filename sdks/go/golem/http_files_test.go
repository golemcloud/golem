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
	"os"
	"reflect"
	"strings"
	"testing"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
)

// The shared HTTP handler corpus every SDK and the host check themselves
// against. Its "mapping" suite is the file-mapping contract.
const httpCorpus = "../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"

type corpusCase struct {
	ID    string `json:"id"`
	Suite string `json:"suite"`
	Input struct {
		Mappings [][2]string `json:"mappings"`
	} `json:"input"`
	Expect struct {
		Error    string            `json:"error"`
		Compiled []json.RawMessage `json:"compiled"`
	} `json:"expect"`
}

func TestFileMappingsFollowTheSharedCorpus(t *testing.T) {
	raw, err := os.ReadFile(httpCorpus)
	if err != nil {
		t.Fatalf("the shared corpus: %v", err)
	}
	var corpus struct {
		Cases []corpusCase `json:"cases"`
	}
	if err := json.Unmarshal(raw, &corpus); err != nil {
		t.Fatal(err)
	}
	checked := 0
	for _, c := range corpus.Cases {
		if c.Suite != "mapping" {
			continue
		}
		checked++
		t.Run(c.ID, func(t *testing.T) {
			mappings := make([]FileMapping, len(c.Input.Mappings))
			for i, m := range c.Input.Mappings {
				mappings[i] = FileMapping{Route: m[0], Path: m[1]}
			}
			compiled, errs := compileFileMappings(mappings)
			if c.Expect.Error != "" {
				if len(errs) == 0 || !strings.HasSuffix(errs[0], ": "+c.Expect.Error) {
					t.Fatalf("want %s, got %v", c.Expect.Error, errs)
				}
				return
			}
			if len(errs) > 0 {
				t.Fatalf("unexpected errors: %v", errs)
			}
			var want []any
			for _, w := range c.Expect.Compiled {
				var v any
				_ = json.Unmarshal(w, &v)
				want = append(want, v)
			}
			if got := corpusForm(compiled); !reflect.DeepEqual(got, want) {
				t.Fatalf("compiled to %v, want %v", got, want)
			}
		})
	}
	if checked == 0 {
		t.Fatal("the corpus has no mapping cases")
	}
}

// corpusForm renders compiled mappings the way the corpus spells them.
func corpusForm(ms []common.FileMapping) []any {
	strs := func(ss []string) []any {
		out := make([]any, len(ss))
		for i, s := range ss {
			out[i] = s
		}
		return out
	}
	var out []any
	for _, m := range ms {
		switch m.Tag() {
		case common.FileMappingExact:
			e := m.Exact()
			out = append(out, map[string]any{"Exact": map[string]any{"public_path": strs(e.PublicPath), "file_path": e.FilePath}})
		case common.FileMappingSubtree:
			s := m.Subtree()
			out = append(out, map[string]any{"Subtree": map[string]any{"public_prefix": strs(s.PublicPrefix), "filesystem_root": s.FilesystemRoot}})
		}
	}
	return out
}

func TestExposeFilesPublishesFilesystemBindings(t *testing.T) {
	e := agent("Owner", &Mount{
		Path:        "/users/{id}",
		ExposeFiles: []FileMapping{{Route: "/avatar", Path: "/avatar.png"}, {Route: "/*", Path: "/public/$1"}},
	}, fields("id"))
	mount, _, errs := buildHTTP(e)
	if len(errs) > 0 {
		t.Fatalf("unexpected errors: %v", errs)
	}
	got := corpusForm(mount.Some().FilesystemBindings)
	if len(got) != 2 || !strings.Contains(stringOf(got[0]), "avatar.png") || !strings.Contains(stringOf(got[1]), "/public") {
		t.Fatalf("bindings = %v", got)
	}
	if len(mount.Some().StaticBindings) != 0 {
		t.Fatalf("an ordinary agent has no static bindings")
	}
}

func stringOf(v any) string {
	b, _ := json.Marshal(v)
	return string(b)
}

// The owner rules mirror the corpus's metadata-live cases: only a durable,
// non-phantom agent whose identity comes wholly from the mount path.
func TestExposeFilesNeedsAnAddressableOwner(t *testing.T) {
	files := []FileMapping{{Route: "/*", Path: "/public/$1"}}
	cases := []struct {
		name  string
		entry *agentEntry
		want  string
	}{
		{"unbound identity", func() *agentEntry {
			e := agent("A", &Mount{Path: "/users/{id}", ExposeFiles: files}, fields("id", "locale"))
			return e
		}(), "unbound-constructor"},
		{"phantom", agent("A", &Mount{Path: "/files", PhantomAgent: true, ExposeFiles: files}, nil), "filesystem-owner"},
		{"ephemeral", func() *agentEntry {
			e := agent("A", &Mount{Path: "/files", ExposeFiles: files}, nil)
			e.mode = common.AgentModeEphemeral
			return e
		}(), "filesystem-owner"},
		{"catch-all capture", agent("A", &Mount{Path: "/files/{*rest}", ExposeFiles: files}, fields("rest")), "unbound-constructor"},
		{"bad mapping", agent("A", &Mount{Path: "/files", ExposeFiles: []FileMapping{{Route: "/a", Path: "one"}}}, nil), "target-path"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			_, _, errs := buildHTTP(c.entry)
			if !anyErrContains(errs, c.want) {
				t.Fatalf("want an error mentioning %q, got %v", c.want, errs)
			}
		})
	}
}

func anyErrContains(errs []definitionError, want string) bool {
	for _, e := range errs {
		if strings.Contains(e.detail, want) {
			return true
		}
	}
	return false
}
