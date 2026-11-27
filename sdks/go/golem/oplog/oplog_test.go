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

package oplog

import (
	"os"
	"regexp"
	"slices"
	"strings"
	"testing"

	oplogwit "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_oplog"
)

// TestEveryEntryCaseHasAConstant — cases.go aliases the generated case tags;
// regenerating the bindings with a new case must not leave it out.
func TestEveryEntryCaseHasAConstant(t *testing.T) {
	names := func(path string, re *regexp.Regexp) []string {
		t.Helper()
		src, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		var out []string
		for _, m := range re.FindAllStringSubmatch(string(src), -1) {
			// The cases of the attribution variant share the prefix of the entry cases.
			if !strings.HasPrefix(m[1], "Attribution") {
				out = append(out, m[1])
			}
		}
		slices.Sort(out)
		return out
	}
	generated := names("../internal/wit/golem_api_oplog/wit_bindings.go", regexp.MustCompile(`(?m)^\s*PublicOplogEntry(\w+)\s+uint8 = \d+`))
	aliased := names("cases.go", regexp.MustCompile(`(?m)^\s*(\w+) = oplogwit\.PublicOplogEntry\w+`))
	if len(generated) == 0 || !slices.Equal(generated, aliased) {
		t.Fatalf("cases.go is out of date with the bindings:\n generated %v\n aliased   %v", generated, aliased)
	}
}

func TestEntriesSwitchOnTheCaseConstants(t *testing.T) {
	for _, kind := range []OwnerKind{OwnerKindComponentAgent, OwnerKindEphemeralExternalTool, OwnerKindOplogProcessor} {
		e := oplogwit.MakePublicOplogEntryCreate(CreateParameters{OwnerKind: kind})
		if e.Create().OwnerKind != kind {
			t.Fatalf("owner kind = %d, want %d", e.Create().OwnerKind, kind)
		}
	}
	e := oplogwit.MakePublicOplogEntryLog(LogParameters{Message: "hello"})
	switch e.Tag() {
	case AgentInvocationStarted, Error:
		t.Fatal("wrong case")
	case Log:
		if e.Log().Message != "hello" {
			t.Fatalf("message = %q", e.Log().Message)
		}
	default:
		t.Fatalf("unexpected tag %d", e.Tag())
	}
}

func TestCheckpointPreservesSourceComponentRevision(t *testing.T) {
	const revision uint64 = 9007199254740993
	entry := oplogwit.MakePublicOplogEntryOplogProcessorCheckpoint(oplogwit.OplogProcessorCheckpointParameters{
		SourceComponentRevision: revision,
	})
	if got := entry.OplogProcessorCheckpoint().SourceComponentRevision; got != revision {
		t.Fatalf("source component revision = %d, want %d", got, revision)
	}
}

func TestAttributionNamesTheNestedEntityInvocation(t *testing.T) {
	invocation := func(name string, start uint64) EntityInvocation {
		return EntityInvocation{Entity: AgentEntity{Name: name}, StartIndex: start}
	}
	attributions := []Attribution{
		oplogwit.MakePublicOplogEntryAttributionAgent(),
		oplogwit.MakePublicOplogEntryAttributionEntity(EntityInvocationContext{
			Invocation: invocation("lookup", 7),
			Ancestors:  []EntityInvocation{invocation("audit", 3)},
		}),
	}
	var got []string
	for _, a := range attributions {
		switch a.Tag() {
		case AttributionAgent:
			got = append(got, "agent")
		case AttributionEntity:
			c := a.Entity()
			got = append(got, c.Ancestors[0].Entity.Name+">"+c.Invocation.Entity.Name)
		}
	}
	if !slices.Equal(got, []string{"agent", "audit>lookup"}) {
		t.Fatalf("attributions = %v", got)
	}
}
