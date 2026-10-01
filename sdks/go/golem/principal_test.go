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
	"testing"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

type pID struct {
	Name  string
	Owner Principal
}

type pState struct{ owner Principal }

type pChargeIn struct {
	Amount    int64
	Principal Principal
}

func definePrincipalAgent(d *definitions) *AgentDefinition[pID, NoConfig] {
	def := defineAgentInto[pID, NoConfig](d, Spec{Name: "P"})
	impl := implementInto[pID, pState, NoConfig](d, def, simpleNewState[pID, pState](func(id pID) *pState {
		return &pState{owner: id.Owner}
	}), false)
	impl.Handle(def.Method[pChargeIn, string]("charge"), func(ctx *Context[pState], in pChargeIn) string {
		return fmt.Sprintf("%d %T %T %T", in.Amount, in.Principal, ctx.Principal(), ctx.State.owner)
	})
	impl.Handle(def.Method[pChargeIn, int64]("balance", ReadOnly(NoCache())), func(*Context[pState], pChargeIn) int64 { return 0 })
	impl.Handle(def.Method[Unit, int64]("total", ReadOnly(NoCache())), func(*Context[pState], Unit) int64 { return 0 })
	return def
}

// TestPrincipalFieldsAreHostFilled — a Principal field is published as the
// host's auto-injected principal, and a read-only method taking one caches per
// principal.
func TestPrincipalFieldsAreHostFilled(t *testing.T) {
	withDefs(t, func(d *definitions) {
		definePrincipalAgent(d)
		noDefErrs(t, d)
		agentTypes, _ := d.discover()
		at := agentTypes[0]

		ctor := at.Constructor.InputSchema.Parameters()
		if len(ctor) != 2 || ctor[0].Source.Tag() != common.FieldSourceUserSupplied ||
			ctor[1].Name != "owner" || ctor[1].Source.Tag() != common.FieldSourceAutoInjected {
			t.Fatalf("constructor parameters = %+v", ctor)
		}
		methods := map[string]common.AgentMethod{}
		for _, m := range at.Methods {
			methods[m.Name] = m
		}
		charge := methods["charge"].InputSchema.Parameters()
		if len(charge) != 2 || charge[1].Name != "principal" || charge[1].Source.Tag() != common.FieldSourceAutoInjected {
			t.Fatalf("charge parameters = %+v", charge)
		}
		if !methods["balance"].ReadOnly.Some().UsesPrincipal {
			t.Error("a read-only method taking the principal must cache per principal")
		}
		if methods["total"].ReadOnly.Some().UsesPrincipal {
			t.Error("a read-only method without a principal must not cache per principal")
		}
	})
}

// TestPrincipalsReachTheAgent — the constructor's principal becomes the
// agent's, the invocation's fills the method's field, and neither has a value
// on the wire.
func TestPrincipalsReachTheAgent(t *testing.T) {
	withDefs(t, func(d *definitions) {
		definePrincipalAgent(d)
		e := d.agents["P"]
		creator := GolemUserPrincipal{}
		idVal := reflect.New(e.idType).Elem()
		if err := decodeParams(params(types.MakeSchemaValueNodeStringValue("p")), e.idFields, idVal, creator); err != nil {
			t.Fatal(err)
		}
		inst := &instance{def: e, agentID: `P("p")`, principal: creator}
		inst.state = e.newState(idVal, inst.agentID, creator)

		out, err := e.methods["charge"].invoke(inst, AgentPrincipal{AgentID: AgentID{AgentID: "Caller()"}}, params(types.MakeSchemaValueNodeS64Value(5)))
		if err != nil {
			t.Fatal(err)
		}
		got := out.ValueNodes[out.Root].StringValue()
		want := "5 golem.AgentPrincipal golem.GolemUserPrincipal golem.GolemUserPrincipal"
		if got != want {
			t.Fatalf("charge = %q, want %q", got, want)
		}
	})
}

// TestCallersNeverSendThePrincipal — a caller's own principal field is not
// part of the call.
func TestCallersNeverSendThePrincipal(t *testing.T) {
	m := MethodDef[pID, pChargeIn, string]{name: "charge"}
	tree, err := m.encodeInput(pChargeIn{Amount: 5, Principal: AnonymousPrincipal{}})
	if err != nil {
		t.Fatal(err)
	}
	if n := len(tree.ValueNodes[tree.Root].RecordValue()); n != 1 {
		t.Fatalf("the call carries %d values, want only the amount", n)
	}
}

func TestPrincipalConvertsFromTheHost(t *testing.T) {
	for _, p := range []Principal{
		OidcPrincipal{Sub: "s", Issuer: "i", Email: Some("e"), EmailVerified: Some(true), Claims: "{}"},
		AgentPrincipal{AgentID: AgentID{ComponentID: UUID{1}, AgentID: "A()"}},
		GolemUserPrincipal{AccountID: UUID{2}},
		AnonymousPrincipal{},
	} {
		if back := principalFromWit(principalToWit(p)); back != p {
			t.Errorf("%#v came back as %#v", p, back)
		}
	}
}

func TestMisusePrincipalOutsideAParameterList(t *testing.T) {
	type Nested struct{ P Principal }
	type NestedIn struct{ N Nested }
	type ID struct{ Name string }
	type St struct{}
	cases := map[string]func(*definitions, *AgentDefinition[ID, NoConfig], *AgentImpl[ID, St, NoConfig]){
		"result": func(_ *definitions, def *AgentDefinition[ID, NoConfig], impl *AgentImpl[ID, St, NoConfig]) {
			impl.Handle(def.Method[Unit, Principal]("m"), func(*Context[St], Unit) Principal { return nil })
		},
		"nested": func(_ *definitions, def *AgentDefinition[ID, NoConfig], impl *AgentImpl[ID, St, NoConfig]) {
			impl.Handle(def.Method[NestedIn, Unit]("m"), func(*Context[St], NestedIn) Unit { return Unit{} })
		},
		"list": func(_ *definitions, def *AgentDefinition[ID, NoConfig], impl *AgentImpl[ID, St, NoConfig]) {
			type ListIn struct{ Ps []Principal }
			impl.Handle(def.Method[ListIn, Unit]("m"), func(*Context[St], ListIn) Unit { return Unit{} })
		},
	}
	for name, declare := range cases {
		t.Run(name, func(t *testing.T) {
			withDefs(t, func(d *definitions) {
				def := defineAgentInto[ID, NoConfig](d, Spec{Name: "A"})
				impl := implementInto[ID, St, NoConfig](d, def, simpleNewState[ID, St](func(ID) *St { return &St{} }), false)
				declare(d, def, impl)
				mustDefErr(t, d, "only valid as a direct field")
			})
		})
	}
}

func TestMisuseBindingThePrincipalToARoute(t *testing.T) {
	type ID struct{ Name string }
	type St struct{}
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[ID, NoConfig](d, Spec{Name: "A", HTTP: &Mount{Path: "/a/{name}"}})
		impl := implementInto[ID, St, NoConfig](d, def, simpleNewState[ID, St](func(ID) *St { return &St{} }), false)
		impl.Handle(def.Method[pChargeIn, string]("charge", HTTP(GET("/charge/{principal}"))),
			func(*Context[St], pChargeIn) string { return "" })
		mustDefErr(t, d, "names the principal")
	})
}

func TestARouterRequestCarriesItsPrincipal(t *testing.T) {
	r := &HTTPRouter[NoConfig]{name: "R"}
	want := OidcPrincipal{Sub: "u", Issuer: "i"}
	if got := r.Principal(r.scope(want)); got != want {
		t.Fatalf("Principal = %#v, want %#v", got, want)
	}
	if got := r.Principal(r.scope(nil)); got != nil {
		t.Fatalf("outside a request the principal is nil, got %#v", got)
	}
}
