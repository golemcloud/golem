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

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
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
	withDefs(t, func(d *definitions) {
		d.recordErr("", "", "global problem")          // affects every agent
		d.recordErr("Counter", "", "counter problem")  // only Counter
		d.recordErr("Ledger", "add", "ledger problem") // only Ledger

		got := agentDefErrors(d.errs, "Counter")
		if !strings.Contains(got, "global problem") || !strings.Contains(got, "counter problem") {
			t.Errorf("Counter errors missing global or own: %q", got)
		}
		if strings.Contains(got, "ledger problem") {
			t.Errorf("Counter errors leaked another agent's: %q", got)
		}

		// An agent with no errors of its own still inherits global ones.
		if got := agentDefErrors(d.errs, "Unrelated"); !strings.Contains(got, "global problem") || strings.Contains(got, "counter problem") {
			t.Errorf("Unrelated agent errors = %q", got)
		}
	})
}

func TestPublicAccessors(t *testing.T) {
	if got := (&AgentDefinition[struct{}, NoConfig]{name: "N"}).Name(); got != "N" {
		t.Errorf("AgentDefinition.Name = %q", got)
	}
	md := (&AgentDefinition[struct{}, NoConfig]{name: "N"}).Method[struct{}, struct{}]("m", Desc("d"), HTTP(GET("/x")))
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
	withDefs(t, func(d *definitions) {
		d.recordErr("Counter", "", "first")
		d.recordErr("Ledger", "add", "second")
		got := allDefErrors(d.errs)
		if !strings.Contains(got, "2 agent definition error(s)") {
			t.Errorf("missing count: %q", got)
		}
		if !strings.Contains(got, "first") || !strings.Contains(got, "second") {
			t.Errorf("missing a problem: %q", got)
		}
	})
}

// TestValidAgentFinalizesAndPublishesItsMount — The whole register → discover → publish pipeline, exercised natively over an
// explicit definitions: a valid agent with a mounted method discovers cleanly and
// publishes the mount and endpoint. This dumps exactly what discovery produces,
// with no global state.
func TestValidAgentFinalizesAndPublishesItsMount(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	type AddIn struct{ By int64 }
	withDefs(t, func(d *definitions) {
		a := defineAgentInto[Id, NoConfig](d,
			Spec{Name: "Counter", HTTP: &Mount{Path: "/c/{name}"}})
		add := a.Method[AddIn, int64]("add", HTTP(POST("/add?by={by}")))
		impl := implementInto[Id, St, NoConfig](d, a, simpleNewState[Id, St](func(Id) *St { return &St{} }), false)
		impl.Handle(add, func(*Context[St], AddIn) int64 { return 0 })

		types, errs := d.discover()
		if len(errs) != 0 {
			t.Fatalf("a valid agent produced errors: %v", errs)
		}
		if len(types) != 1 {
			t.Fatalf("expected 1 agent type, got %d", len(types))
		}
		at := types[0]
		if !at.HttpMount.IsSome() {
			t.Fatal("mount was not published")
		}
		if len(at.Methods) != 1 || at.Methods[0].Name != "add" {
			t.Fatalf("methods = %+v", at.Methods)
		}
		if len(at.Methods[0].HttpEndpoint) != 1 {
			t.Fatalf("endpoint not published: %+v", at.Methods[0])
		}
	})
}

// TestSeparateDefinitionsDoNotLeak — The same agent name registered into two separate definitions both discover
// cleanly — isolation is inherent now that registration is instance-based (no
// shared global to leak between tests).
func TestSeparateDefinitionsDoNotLeak(t *testing.T) {
	register := func() []definitionError {
		type Id struct{ Name string }
		type St struct{}
		var errs []definitionError
		withDefs(t, func(d *definitions) {
			def := defineAgentInto[Id, NoConfig](d, Spec{Name: "Solo"})
			impl := implementInto[Id, St, NoConfig](d, def, simpleNewState[Id, St](func(Id) *St { return &St{} }), false)
			impl.Handle(def.Method[Unit, Unit]("ping"), func(*Context[St], Unit) Unit { return Unit{} })
			_, errs = d.discover()
		})
		return errs
	}
	if e := register(); len(e) != 0 {
		t.Fatalf("first registration: %v", e)
	}
	if e := register(); len(e) != 0 {
		t.Fatalf("second registration: %v", e)
	}
}

type DepID struct{ Name string }
type BossID struct{ Boss string }
type LonelyID struct{ Lonely string }
type depState struct{}

// TestAgentDependenciesArePublished — an agent publishes the agents of its
// component it depends on, and one the component does not define is a
// definition error.
func TestAgentDependenciesArePublished(t *testing.T) {
	d := newDefinitions()
	worker := defineAgentInto[DepID, NoConfig](d, Spec{Name: "Worker", Description: "Does the work"})
	impl := implementInto[DepID, depState, NoConfig](d, worker, simpleNewState[DepID, depState](func(DepID) *depState { return &depState{} }), false)
	impl.Handle(worker.Method[Unit, string]("work"), func(*Context[depState], Unit) string { return "" })

	boss := defineAgentInto[BossID, NoConfig](d, Spec{Name: "Boss", Dependencies: []AgentDependency{worker}})
	bossImpl := implementInto[BossID, depState, NoConfig](d, boss, simpleNewState[BossID, depState](func(BossID) *depState { return &depState{} }), false)
	bossImpl.Handle(boss.Method[Unit, Unit]("run"), func(*Context[depState], Unit) Unit { return Unit{} })

	found, errs := d.discover()
	if len(errs) > 0 {
		t.Fatalf("definition errors: %s", allDefErrors(errs))
	}
	var deps []common.AgentDependency
	for _, at := range found {
		if at.TypeName == "Boss" {
			deps = at.Dependencies
		}
	}
	if len(deps) != 1 || deps[0].TypeName != "Worker" {
		t.Fatalf("dependencies %+v", deps)
	}
	if deps[0].Description.Some() != "Does the work" || len(deps[0].Methods) != 1 || deps[0].Methods[0].Name != "work" {
		t.Errorf("dependency records %+v", deps)
	}

	other := newDefinitions()
	stranger := defineAgentInto[DepID, NoConfig](other, Spec{Name: "Stranger"})
	lonely := defineAgentInto[LonelyID, NoConfig](d, Spec{Name: "Lonely", Dependencies: []AgentDependency{stranger}})
	lonelyImpl := implementInto[LonelyID, depState, NoConfig](d, lonely, simpleNewState[LonelyID, depState](func(LonelyID) *depState { return &depState{} }), false)
	lonelyImpl.Handle(lonely.Method[Unit, Unit]("run"), func(*Context[depState], Unit) Unit { return Unit{} })
	mustDefErr(t, d, "depends on Stranger, which this component does not define")
}
