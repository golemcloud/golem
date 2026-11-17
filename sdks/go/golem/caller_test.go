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
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

type textFormVectors struct {
	RoundTrip []struct {
		Name    string `json:"name"`
		Issuer  string `json:"issuer"`
		Subject string `json:"subject"`
		Text    string `json:"text"`
	} `json:"roundTrip"`
	Reject []struct {
		Name string `json:"name"`
		Text string `json:"text"`
	} `json:"reject"`
}

func loadVectors(t *testing.T) textFormVectors {
	t.Helper()
	raw, err := os.ReadFile("../../../test-data/caller-identity/text-form-v1.json")
	if err != nil {
		t.Fatal(err)
	}
	var v textFormVectors
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	return v
}

func TestPrincipalRefTextFormMatchesTheSharedVectors(t *testing.T) {
	v := loadVectors(t)
	if len(v.RoundTrip) == 0 || len(v.Reject) == 0 {
		t.Fatal("no vectors")
	}
	for _, c := range v.RoundTrip {
		ref := PrincipalRef{Issuer: c.Issuer, Subject: c.Subject}
		if got := ref.String(); got != c.Text {
			t.Errorf("%s: printed %q, want %q", c.Name, got, c.Text)
		}
		parsed, err := ParsePrincipalRef(c.Text)
		if err != nil || parsed != ref {
			t.Errorf("%s: parsed %+v, %v", c.Name, parsed, err)
		}
	}
	for _, c := range v.Reject {
		if _, err := ParsePrincipalRef(c.Text); err == nil {
			t.Errorf("%s: %q parsed", c.Name, c.Text)
		}
	}
}

func TestPrincipalRefEqualsIsByteEquality(t *testing.T) {
	base := PrincipalRef{Issuer: "https://login.example/", Subject: "Subject-1"}
	if !base.Equals(PrincipalRef{Issuer: "https://login.example/", Subject: "Subject-1"}) {
		t.Error("equal references differ")
	}
	for _, other := range []PrincipalRef{
		{Issuer: "https://login.example", Subject: "Subject-1"},
		{Issuer: "https://login.example/", Subject: "subject-1"},
		{Issuer: "https://login.example/", Subject: "Subject-1 "},
	} {
		if base.Equals(other) {
			t.Errorf("%+v equals the base", other)
		}
	}
}

func pair(subject string) common.PrincipalRef {
	return common.PrincipalRef{Issuer: "https://login.example", Subject: subject}
}

func witUser(subject, claims string) common.Principal {
	return common.MakePrincipalUser(common.UserPrincipal{
		Pair: pair(subject), Claims: claims,
		AuthenticatedAt: types.Datetime{Seconds: 1_700_000_000, Nanoseconds: 5},
	})
}

func witAgent() common.Principal {
	return common.MakePrincipalAgent(common.AgentPrincipal{
		Pair:          pair("agent:2/3"),
		AgentId:       types.AgentId{AgentId: `Relay("r")`},
		AgentType:     "Relay",
		EnvironmentId: types.EnvironmentId{},
	})
}

func TestCallerReadsBothRolesTokensAndPrintsWithoutClaims(t *testing.T) {
	c := callerFromWit(common.Caller{
		Original:  witUser("u-1", `{"role":"admin","note":"secret"}`),
		Immediate: witAgent(),
		Tokens: []common.TokenProvenance{
			{Id: types.Uuid{HighBits: 9, LowBits: 7}, Maker: witTypes.Some(pair("account:1"))},
			{Id: types.Uuid{HighBits: 10, LowBits: 7}, Maker: witTypes.None[common.PrincipalRef]()},
		},
	})
	if c.Original.Kind() != KindUser || c.Immediate.Kind() != KindAgent || len(c.Tokens) != 2 {
		t.Fatalf("caller %+v", c)
	}
	if m, ok := c.Tokens[0].Maker.Get(); !ok || m.Subject != "account:1" || c.Tokens[1].Maker.IsSome() {
		t.Errorf("token makers %+v", c.Tokens)
	}
	user := c.Original.(UserPrincipal)
	if user.Claims != `{"role":"admin","note":"secret"}` || user.AuthenticatedAt.Unix() != 1_700_000_000 {
		t.Errorf("user %+v", user)
	}
	text := c.Original.String()
	if text != `user ("https://login.example","u-1")` || strings.Contains(text, "secret") {
		t.Errorf("user prints as %q", text)
	}
	if got := c.Immediate.(AgentPrincipal); got.AgentType != "Relay" || got.AgentID.AgentID != `Relay("r")` {
		t.Errorf("agent %+v", got)
	}
	if AnonymousCaller().Original.String() != "anonymous" {
		t.Error("anonymous does not print as anonymous")
	}
}

func TestPrincipalEqualsIsIdentityAndAnonymousEqualsNothing(t *testing.T) {
	a := principalFromWit(witUser("u-1", "{}"))
	same := principalFromWit(witUser("u-1", `{"iat":2}`))
	other := principalFromWit(witUser("u-2", "{}"))
	nobody := principalFromWit(common.MakePrincipalAnonymous())
	if !a.Equals(same) || a.Equals(other) || !a.EqualsRef(PrincipalRef{Issuer: "https://login.example", Subject: "u-1"}) {
		t.Error("identity comparison is wrong")
	}
	if nobody.Equals(nobody) || nobody.Equals(a) || a.Equals(nobody) || nobody.EqualsRef(PrincipalRef{}) {
		t.Error("anonymous equals something")
	}
}

type liveIn struct{ Who Principal }
type liveCallerIn struct{ C Caller }
type liveTokenIn struct{ T []TokenProvenance }
type refIn struct {
	Who PrincipalRef
	All []PrincipalRef
}
type liveID struct{ Name string }
type liveState struct{}

func TestLiveValuesAreRefusedInEveryPosition(t *testing.T) {
	d := newDefinitions()
	for name, typ := range map[string]reflect.Type{
		"principal": reflect.TypeFor[Principal](), "caller": reflect.TypeFor[Caller](),
		"token provenance": reflect.TypeFor[TokenProvenance](), "concrete principal": reflect.TypeFor[UserPrincipal](),
	} {
		if msg := d.Compile(typ).Invalid; !strings.Contains(msg, "Principal and Caller cannot appear in a method, constructor or tool signature") {
			t.Errorf("%s: message %q", name, msg)
		}
	}
	// Reached through a field of a method's input, a list or a result.
	for name, in := range map[string]func(*definitions, *AgentDefinition[liveID, NoConfig]){
		"input field": func(d *definitions, def *AgentDefinition[liveID, NoConfig]) {
			implementInto[liveID, liveState, NoConfig](d, def, simpleNewState[liveID, liveState](func(liveID) *liveState { return &liveState{} }), false).
				Handle(def.Method[liveIn, Unit]("m"), func(*Context[liveState], liveIn) Unit { return Unit{} })
		},
		"input list": func(d *definitions, def *AgentDefinition[liveID, NoConfig]) {
			implementInto[liveID, liveState, NoConfig](d, def, simpleNewState[liveID, liveState](func(liveID) *liveState { return &liveState{} }), false).
				Handle(def.Method[liveTokenIn, Unit]("m"), func(*Context[liveState], liveTokenIn) Unit { return Unit{} })
		},
		"result": func(d *definitions, def *AgentDefinition[liveID, NoConfig]) {
			implementInto[liveID, liveState, NoConfig](d, def, simpleNewState[liveID, liveState](func(liveID) *liveState { return &liveState{} }), false).
				Handle(def.Method[Unit, liveCallerIn]("m"), func(*Context[liveState], Unit) liveCallerIn { return liveCallerIn{} })
		},
	} {
		t.Run(name, func(t *testing.T) {
			withDefs(t, func(d *definitions) {
				in(d, defineAgentInto[liveID, NoConfig](d, Spec{Name: "Live"}))
				mustDefErr(t, d, "Principal and Caller cannot appear in a method, constructor or tool signature")
			})
		})
	}
}

func TestPrincipalRefTravelsAsTheNamedRecord(t *testing.T) {
	d := newDefinitions()
	if got := d.TypeID(reflect.TypeFor[PrincipalRef]()); got != "principal-ref" {
		t.Errorf("type id %q", got)
	}
	if msg := d.Compile(reflect.TypeFor[refIn]()).Invalid; msg != "" {
		t.Errorf("a PrincipalRef is plain data, got %q", msg)
	}
}

func TestAContextReadsItsCallerLazily(t *testing.T) {
	reads := 0
	prev := currentCaller
	currentCaller = func() common.Caller {
		reads++
		return common.Caller{Original: witUser("u-1", "{}"), Immediate: witAgent()}
	}
	t.Cleanup(func() { currentCaller = prev })
	read, _ := lazyCaller()
	ctx := &Context[liveState]{caller: read}
	if reads != 0 {
		t.Fatal("the host was read before the caller was used")
	}
	if ctx.Caller().Original.Kind() != KindUser || ctx.Caller().Immediate.Kind() != KindAgent || reads != 1 {
		t.Errorf("reads %d", reads)
	}
	if (&Context[liveState]{}).Caller().Original.Kind() != KindAnonymous {
		t.Error("a context without an invocation is not anonymous")
	}
}

func TestAContextRefusesAFirstReadAfterItsCallEnded(t *testing.T) {
	var current = "A"
	prev := currentCaller
	currentCaller = func() common.Caller {
		return common.Caller{Original: witUser(current, "{}"), Immediate: witAgent()}
	}
	t.Cleanup(func() { currentCaller = prev })
	subject := func(c Caller) string {
		ref, _ := c.Original.Ref()
		return ref.Subject
	}

	// A context read during its call gives that call's caller.
	read, end := lazyCaller()
	if got := subject(read()); got != "A" {
		t.Errorf("read during the call = %q", got)
	}
	// A context read during its call keeps the first value, even after the call.
	current = "B"
	end()
	if got := subject(read()); got != "A" {
		t.Errorf("read after the call = %q, want the first value", got)
	}

	// A context first read after its call ended refuses.
	late, lateEnd := lazyCaller()
	lateEnd()
	defer func() {
		if r := recover(); r == nil || !strings.Contains(r.(string), "belongs to a call that has ended") {
			t.Errorf("a first read after the call ended did not refuse: %v", r)
		}
	}()
	late()
}
