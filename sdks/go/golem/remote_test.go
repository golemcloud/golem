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
	"reflect"
	"strings"
	"testing"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
)

type RemoteCounterID struct{ Name string }

type RemoteAddIn struct{ By int64 }

// TestRemoteAgentIsNotPublished — the whole reason a full agent client exists: a
// component that CALLS an agent must not advertise it as one it provides, and
// must not fail its own definition check for not implementing it.
func TestRemoteAgentIsNotPublished(t *testing.T) {
	withDefs(t, func(d *definitions) {
		defineFullAgentClientInto[RemoteCounterID, NoConfig](d, "CounterAgent", AgentClientSpec{})

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

		defineFullAgentClientInto[RemoteCounterID, NoConfig](d, "CounterAgent", AgentClientSpec{})

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
		defineFullAgentClientInto[RemoteCounterID, NoConfig](d, "CounterAgent", AgentClientSpec{})

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
		defineFullAgentClientInto[RemoteCounterID, NoConfig](d, "CounterAgent", AgentClientSpec{})
		if _, claimed := d.idToAgent[reflect.TypeFor[RemoteCounterID]()]; claimed {
			t.Error("a remote declaration claimed its Id type")
		}
	})
}

func TestFullAgentClientErrors(t *testing.T) {
	t.Run("empty name", func(t *testing.T) {
		withDefs(t, func(d *definitions) {
			defineFullAgentClientInto[RemoteCounterID, NoConfig](d, "", AgentClientSpec{})
			mustDefErr(t, d, "requires a non-empty name")
		})
	})

	t.Run("non-struct id", func(t *testing.T) {
		withDefs(t, func(d *definitions) {
			defineFullAgentClientInto[string, NoConfig](d, "CounterAgent", AgentClientSpec{})
			mustDefErr(t, d, "Id must be a struct")
		})
	})

	t.Run("declared twice", func(t *testing.T) {
		withDefs(t, func(d *definitions) {
			defineFullAgentClientInto[RemoteCounterID, NoConfig](d, "CounterAgent", AgentClientSpec{})
			defineFullAgentClientInto[RemoteCounterID, NoConfig](d, "CounterAgent", AgentClientSpec{})
			mustDefErr(t, d, "agent client already defined")
		})
	})

	// Calling an agent this component implements is a mistake worth naming: the
	// local definition is right there and carries the config surface too.
	t.Run("shadowing a local agent", func(t *testing.T) {
		withDefs(t, func(d *definitions) {
			defineAgentInto[RemoteCounterID, NoConfig](d, Spec{Name: "Local"})
			defineFullAgentClientInto[RemoteCounterID, NoConfig](d, "Local", AgentClientSpec{})
			mustDefErr(t, d, "is defined by this component")
		})
	})
}

// TestRemoteMethodDescriptorMatchesTheLocalOne — the descriptor is the contract
// Call invokes through, so it must carry the same fields either way.
func TestRemoteMethodDescriptorMatchesTheLocalOne(t *testing.T) {
	remote := &FullAgentClient[RemoteCounterID, NoConfig]{name: "CounterAgent"}
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
			defineFullAgentClientInto[RemoteCounterID, NoConfig](d, "Both", AgentClientSpec{})
			defineAgentInto[RemoteCounterID, NoConfig](d, Spec{Name: "Both"})
			mustDefErr(t, d, "rather than a client definition")
		})
	})

	t.Run("local first", func(t *testing.T) {
		withDefs(t, func(d *definitions) {
			defineAgentInto[RemoteCounterID, NoConfig](d, Spec{Name: "Both"})
			defineFullAgentClientInto[RemoteCounterID, NoConfig](d, "Both", AgentClientSpec{})
			mustDefErr(t, d, "rather than a client definition")
		})
	})
}

type RemoteCounterConfig struct{ Threshold int32 }

// TestConfiguredFullAgentClientDeclaresItsConfig — overrides are checked
// against the declared configuration before anything is sent.
func TestConfiguredFullAgentClientDeclaresItsConfig(t *testing.T) {
	withDefs(t, func(d *definitions) {
		defineFullAgentClientInto[RemoteCounterID, RemoteCounterConfig](d, "CounterAgent", AgentClientSpec{})
		e := d.agents["CounterAgent"]
		if !configDeclared(e, []string{"threshold"}) {
			t.Errorf("config declarations are %+v", e.configs)
		}
		if _, err := buildAgentConfig(d, e, []configOverrideFn{func(*definitions) ([]common.TypedAgentConfigValue, error) {
			return []common.TypedAgentConfigValue{{Path: []string{"nope"}}}, nil
		}}); err == nil || !strings.Contains(err.Error(), "not a declared config key") {
			t.Errorf("an undeclared override was accepted: %v", err)
		}
	})
}

// TestEphemeralTargetsHaveNoGet — an ephemeral agent has no durable identity
// to get or bind; only a phantom addresses one.
func TestEphemeralTargetsHaveNoGet(t *testing.T) {
	withDefs(t, func(d *definitions) {
		defineFullAgentClientInto[RemoteCounterID, NoConfig](d, "Request", AgentClientSpec{Mode: Ephemeral})
		e := d.agents["Request"]
		if err := requireIdentity(e, false); err == nil || !strings.Contains(err.Error(), "use NewPhantom") {
			t.Errorf("Get on an ephemeral target gave %v", err)
		}
		if err := requireIdentity(e, true); err != nil {
			t.Errorf("a phantom of an ephemeral target was refused: %v", err)
		}
	})
}

type Pingable struct{}

// TestMethodOnlyClientDeclaresDescriptorsOnly — a method-only client registers
// nothing: its methods are descriptors on its shape type.
func TestMethodOnlyClientDeclaresDescriptorsOnly(t *testing.T) {
	pinger := DefineAgentClient[Pingable]()
	ping := pinger.Method[Unit, string]("ping", Desc("Ping"))
	if ping.Name() != "ping" || ping.desc != "Ping" {
		t.Errorf("descriptor %+v", ping)
	}
}
