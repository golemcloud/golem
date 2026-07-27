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
	"strings"
	"testing"
)

func TestDefinitionErrorMessageIsAttributed(t *testing.T) {
	cases := []struct {
		err  definitionError
		want string
	}{
		{definitionError{detail: "bad type-id"}, "golem: bad type-id"},
		{definitionError{agent: "Counter", detail: "Id must be a struct"}, `golem: agent "Counter": Id must be a struct`},
		{definitionError{agent: "Counter", method: "add", detail: "route var {x} unknown"}, `golem: agent "Counter" method "add": route var {x} unknown`},
	}
	for _, c := range cases {
		if got := c.err.Error(); got != c.want {
			t.Errorf("Error() = %q, want %q", got, c.want)
		}
	}
}

func TestAgentDefErrorsFiltersByAgentAndGlobal(t *testing.T) {
	withIsolatedDefs(t, func() {
		recordDefErr("", "", "global problem")          // affects every agent
		recordDefErr("Counter", "", "counter problem")  // only Counter
		recordDefErr("Ledger", "add", "ledger problem") // only Ledger

		got := agentDefErrors("Counter")
		if !strings.Contains(got, "global problem") || !strings.Contains(got, "counter problem") {
			t.Errorf("Counter errors missing global or own: %q", got)
		}
		if strings.Contains(got, "ledger problem") {
			t.Errorf("Counter errors leaked another agent's: %q", got)
		}

		// An agent with no errors of its own still inherits global ones.
		if got := agentDefErrors("Unrelated"); !strings.Contains(got, "global problem") || strings.Contains(got, "counter problem") {
			t.Errorf("Unrelated agent errors = %q", got)
		}
	})
}

func TestPublicAccessors(t *testing.T) {
	if got := (&Agent[struct{}, struct{}]{name: "N"}).Name(); got != "N" {
		t.Errorf("Agent.Name = %q", got)
	}
	md := DefineMethod[struct{}, struct{}, struct{}]("m", Desc("d"), HTTP(GET("/x")))
	if md.Name() != "m" {
		t.Errorf("MethodDef.Name = %q", md.Name())
	}
	if len(md.endpoints) != 1 {
		t.Errorf("HTTP opt: endpoints = %d, want 1", len(md.endpoints))
	}
	if got := (&Context[int]{agentID: "aid"}).AgentID(); got != "aid" {
		t.Errorf("Context.AgentID = %q", got)
	}
}

func TestDefinitionErrorsIsCleanForAValidComponent(t *testing.T) {
	// The test package's registered agents are all well-formed, so the accessor
	// (which triggers finalize) reports nothing.
	if errs := DefinitionErrors(); len(errs) != 0 {
		t.Fatalf("expected no definition errors, got %v", errs)
	}
}

func TestAllDefErrorsReportsEveryProblem(t *testing.T) {
	withIsolatedDefs(t, func() {
		recordDefErr("Counter", "", "first")
		recordDefErr("Ledger", "add", "second")
		got := allDefErrors()
		if !strings.Contains(got, "2 agent definition error(s)") {
			t.Errorf("missing count: %q", got)
		}
		if !strings.Contains(got, "first") || !strings.Contains(got, "second") {
			t.Errorf("missing a problem: %q", got)
		}
	})
}
