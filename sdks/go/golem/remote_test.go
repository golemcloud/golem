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
	"testing"
)

type RemoteCounterID struct{ Name string }

type RemoteAddIn struct{ By int64 }

// TestRemoteAgentIsNotPublished — the whole reason DeclareRemoteAgent exists: a
// component that CALLS an agent must not advertise it as one it provides, and
// must not fail its own definition check for not implementing it.
func TestRemoteAgentIsNotPublished(t *testing.T) {
	withDefs(t, func(d *definitions) {
		declareRemoteAgentInto[RemoteCounterID](d, "CounterAgent")

		types, errs := d.discover()
		if len(errs) != 0 {
			t.Fatalf("declaring a remote agent produced definition errors: %s", allDefErrors(errs))
		}
		if len(types) != 0 {
			t.Fatalf("a remote agent was published as %d agent type(s); it belongs to another component", len(types))
		}
	})
}

// TestRemoteAgentDoesNotDisturbALocalOne — a component may both implement
// agents and call others; declaring a call target must not change what it
// publishes.
func TestRemoteAgentDoesNotDisturbALocalOne(t *testing.T) {
	type LocalID struct{ Name string }
	type St struct{}

	withDefs(t, func(d *definitions) {
		local := defineAgentInto[LocalID, NoConfig](d, Spec{Name: "Local"})
		m := local.Method[RemoteAddIn, int64]("add")
		impl := implementInto[LocalID, St, NoConfig](d, local,
			simpleNewState[LocalID, St](func(LocalID) *St { return &St{} }), false)
		impl.Handle(m, func(*Context[St], RemoteAddIn) int64 { return 0 })

		declareRemoteAgentInto[RemoteCounterID](d, "CounterAgent")

		types, errs := d.discover()
		if len(errs) != 0 {
			t.Fatalf("definition errors: %s", allDefErrors(errs))
		}
		if len(types) != 1 || types[0].TypeName != "Local" {
			t.Fatalf("published %d agent type(s), want only Local", len(types))
		}
	})
}

// TestRemoteAgentRecordsWhatACallNeeds — Get resolves the target's id fields
// from the registry, so the declaration has to carry them.
func TestRemoteAgentRecordsWhatACallNeeds(t *testing.T) {
	withDefs(t, func(d *definitions) {
		declareRemoteAgentInto[RemoteCounterID](d, "CounterAgent")

		e := d.agents["CounterAgent"]
		if e == nil {
			t.Fatal("a remote agent was not registered at all; Get would not resolve it")
		}
		if !e.remote {
			t.Error("the entry is not marked remote")
		}
		if len(e.idFields) != 1 || e.idFields[0].name != "name" {
			t.Errorf("id fields are %+v, want one named name", e.idFields)
		}
	})
}

// TestRemoteAgentDoesNotClaimItsIdType — that guard stops two LOCAL agents
// sharing an Id type. A target someone else implements says nothing about this
// component's own types.
func TestRemoteAgentDoesNotClaimItsIdType(t *testing.T) {
	withDefs(t, func(d *definitions) {
		declareRemoteAgentInto[RemoteCounterID](d, "CounterAgent")
		if _, claimed := d.idToAgent[typeOf[RemoteCounterID]()]; claimed {
			t.Error("a remote declaration claimed its Id type")
		}
	})
}

func TestDeclareRemoteAgentErrors(t *testing.T) {
	t.Run("empty name", func(t *testing.T) {
		withDefs(t, func(d *definitions) {
			declareRemoteAgentInto[RemoteCounterID](d, "")
			mustDefErr(t, d, "requires a non-empty name")
		})
	})

	t.Run("non-struct id", func(t *testing.T) {
		withDefs(t, func(d *definitions) {
			declareRemoteAgentInto[string](d, "CounterAgent")
			mustDefErr(t, d, "Id must be a struct")
		})
	})

	t.Run("declared twice", func(t *testing.T) {
		withDefs(t, func(d *definitions) {
			declareRemoteAgentInto[RemoteCounterID](d, "CounterAgent")
			declareRemoteAgentInto[RemoteCounterID](d, "CounterAgent")
			mustDefErr(t, d, "remote agent already declared")
		})
	})

	// Calling an agent this component implements is a mistake worth naming: the
	// local definition is right there and carries the config surface too.
	t.Run("shadowing a local agent", func(t *testing.T) {
		withDefs(t, func(d *definitions) {
			defineAgentInto[RemoteCounterID, NoConfig](d, Spec{Name: "Local"})
			declareRemoteAgentInto[RemoteCounterID](d, "Local")
			mustDefErr(t, d, "is defined by this component")
		})
	})
}

// TestRemoteMethodDescriptorMatchesTheLocalOne — the descriptor is the contract
// Call invokes through, so it must carry the same fields either way.
func TestRemoteMethodDescriptorMatchesTheLocalOne(t *testing.T) {
	remote := &Remote[RemoteCounterID]{name: "CounterAgent"}
	m := remote.Method[RemoteAddIn, int64]("add", Desc("Add to the counter"))
	if m.Name() != "add" {
		t.Errorf("name %q", m.Name())
	}
	if m.desc != "Add to the counter" || m.descCount != 1 {
		t.Errorf("descriptor is %+v, want the description carried through", m)
	}
}

// TestLocalAndRemoteClashIsReportedEitherWay — package-level var init order
// across packages is unspecified, so a definition and a remote declaration of
// one name can collide from either direction.
func TestLocalAndRemoteClashIsReportedEitherWay(t *testing.T) {
	t.Run("remote first", func(t *testing.T) {
		withDefs(t, func(d *definitions) {
			declareRemoteAgentInto[RemoteCounterID](d, "Both")
			defineAgentInto[RemoteCounterID, NoConfig](d, Spec{Name: "Both"})
			mustDefErr(t, d, "rather than DeclareRemoteAgent")
		})
	})

	t.Run("local first", func(t *testing.T) {
		withDefs(t, func(d *definitions) {
			defineAgentInto[RemoteCounterID, NoConfig](d, Spec{Name: "Both"})
			declareRemoteAgentInto[RemoteCounterID](d, "Both")
			mustDefErr(t, d, "rather than DeclareRemoteAgent")
		})
	})
}
