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
	"reflect"
	"strings"
	"testing"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

type GreeterID struct{ Name string }

type GreetIn struct {
	Greeting string
	Times    int32
}

// snapshotOf derives an agent type the way the host publishes it, so the
// reflection tests read against real metadata rather than a hand-built graph.
func snapshotOf(t *testing.T) ReflectedAgentType {
	t.Helper()
	d := newDefinitions()
	a := defineAgentInto[GreeterID, Unit](d, Spec{
		Name: "Greeter", Description: "Greets people", Mode: Durable,
	})
	m := a.Method[GreetIn, string]("greet", Desc("Greet someone"))
	impl := implementInto[GreeterID, Unit, Unit](d, a, simpleNewState[GreeterID, Unit](
		func(GreeterID) *Unit { return &Unit{} }), false)
	impl.Handle(m, func(_ *Context[Unit], in GreetIn) string {
		return strings.TrimSpace(strings.Repeat(in.Greeting+" ", int(in.Times)))
	})

	found, errs := d.discover()
	if len(errs) > 0 {
		t.Fatalf("definition errors: %s", allDefErrors(errs))
	}
	if len(found) != 1 {
		t.Fatalf("discovered %d agent types, want 1", len(found))
	}
	return ReflectedAgentType{wit: found[0]}
}

// TestSnapshotDescribesTheAgentType — the snapshot is everything a caller gets;
// it has none of the target's Go types.
func TestSnapshotDescribesTheAgentType(t *testing.T) {
	r := snapshotOf(t)
	if r.Name() != "Greeter" || r.Description() != "Greets people" {
		t.Errorf("snapshot is %q/%q", r.Name(), r.Description())
	}

	ctor := r.Constructor().Parameters()
	if len(ctor) != 1 || ctor[0].Name != "name" {
		t.Fatalf("constructor parameters are %+v, want one named name", ctor)
	}

	m, known := r.Method("greet")
	if !known {
		t.Fatal("the snapshot has no greet method")
	}
	if m.Description() != "Greet someone" {
		t.Errorf("method description %q", m.Description())
	}
	params := m.Parameters()
	if len(params) != 2 || params[0].Name != "greeting" || params[1].Name != "times" {
		t.Fatalf("method parameters are %+v", params)
	}
	if _, has := m.Output(); !has {
		t.Error("greet declares no output")
	}

	if _, known := r.Method("absent"); known {
		t.Error("an undeclared method was found")
	}
}

// TestPackParametersBuildsTheInvocationRecord — a reflective call is named
// arguments packed against the snapshot, in the parameter list's own order.
func TestPackParametersBuildsTheInvocationRecord(t *testing.T) {
	r := snapshotOf(t)
	m, _ := r.Method("greet")

	tree, err := m.PackJSON(map[string]any{"greeting": "hi", "times": 2})
	if err != nil {
		t.Fatalf("PackJSON: %v", err)
	}
	root := tree.ValueNodes[tree.Root]
	if root.Tag() != types.SchemaValueNodeRecordValue {
		t.Fatalf("root tag %d, want record", root.Tag())
	}
	if n := len(root.RecordValue()); n != 2 {
		t.Fatalf("record has %d fields, want 2", n)
	}

	// The same tree decodes back into the agent's own Go type.
	var in GreetIn
	fields := newDefinitions().structFields(reflect.TypeFor[GreetIn]())
	if err := decodeParams(tree, fields, reflect.ValueOf(&in).Elem()); err != nil {
		t.Fatalf("the packed tree is not readable by the target: %v", err)
	}
	if in.Greeting != "hi" || in.Times != 2 {
		t.Errorf("decoded %+v", in)
	}
}

// TestPackParametersReportsEveryProblemAtOnce — a caller working from JSON
// should learn about all its mistakes in one go.
func TestPackParametersReportsEveryProblemAtOnce(t *testing.T) {
	r := snapshotOf(t)
	m, _ := r.Method("greet")

	_, err := m.PackJSON(map[string]any{"greetng": "hi", "extra": 1})
	if err == nil {
		t.Fatal("packing malformed arguments succeeded")
	}
	msg := err.Error()
	for _, want := range []string{"greetng", "extra", "greeting", "times"} {
		if !strings.Contains(msg, want) {
			t.Errorf("message does not mention %q: %s", want, msg)
		}
	}
}

func TestPackParametersRoundTrips(t *testing.T) {
	r := snapshotOf(t)
	m, _ := r.Method("greet")

	tree, err := m.PackJSON(map[string]any{"greeting": "hi", "times": 3})
	if err != nil {
		t.Fatalf("PackJSON: %v", err)
	}
	back, err := r.Schema().UnpackParameters(m.Parameters(), tree)
	if err != nil {
		t.Fatalf("UnpackParameters: %v", err)
	}
	if back["greeting"] != "hi" || back["times"] != int64(3) {
		t.Errorf("round trip gave %v", back)
	}
}

// TestMethodJSONSchemaDescribesItsArguments — this is what a model is handed to
// fill in, so it must name every parameter and mark the required ones.
func TestMethodJSONSchemaDescribesItsArguments(t *testing.T) {
	r := snapshotOf(t)
	m, _ := r.Method("greet")

	rendered, err := m.ToJSONSchema(true)
	if err != nil {
		t.Fatalf("ToJSONSchema: %v", err)
	}
	data, _ := json.Marshal(rendered)
	var doc map[string]any
	_ = json.Unmarshal(data, &doc)

	if doc["type"] != "object" || doc["additionalProperties"] != false {
		t.Errorf("schema is %v", doc)
	}
	props, _ := doc["properties"].(map[string]any)
	if _, has := props["greeting"]; !has {
		t.Errorf("properties are %v", props)
	}
	required, _ := doc["required"].([]any)
	if len(required) != 2 {
		t.Errorf("required is %v, want both parameters", required)
	}
	if doc["$schema"] == nil {
		t.Error("the draft marker was asked for but not rendered")
	}
}

// TestAutoInjectedFieldsAreNotAskedOfTheCaller — the host fills the principal
// in, so a caller can neither know it nor override it.
func TestAutoInjectedFieldsAreNotAskedOfTheCaller(t *testing.T) {
	in := common.MakeInputSchemaParameters([]common.NamedField{
		{Name: "name", Source: common.MakeFieldSourceUserSupplied(), Schema: 0},
		{Name: "principal", Source: common.MakeFieldSourceAutoInjected(common.AutoInjectedKindPrincipal), Schema: 0},
	})
	got := userParameters(in)
	if len(got) != 1 || got[0].Name != "name" {
		t.Errorf("parameters are %+v, want only the caller-supplied one", got)
	}
}

// fakeRPC replays a scripted invocation result.
type fakeRPC struct {
	gotMethod string
	tree      types.SchemaValueTree
	has       bool
	err       error
}

func (f *fakeRPC) invokeAndAwait(method string, _ types.SchemaValueTree) (types.SchemaValueTree, bool, error) {
	f.gotMethod = method
	return f.tree, f.has, f.err
}

func TestReflectedClientInvokesAndDecodes(t *testing.T) {
	r := snapshotOf(t)
	m, _ := r.Method("greet")
	out, _ := m.Output()
	result, err := out.PackJSON("hi hi")
	if err != nil {
		t.Fatalf("packing the result: %v", err)
	}

	rpc := &fakeRPC{tree: result, has: true}
	client := &ReflectedAgentClient{agentType: r, agentID: "greeter-1", rpc: rpc}

	got, err := client.InvokeAndAwait("greet", map[string]any{"greeting": "hi", "times": 2})
	if err != nil {
		t.Fatalf("InvokeAndAwait: %v", err)
	}
	if rpc.gotMethod != "greet" {
		t.Errorf("invoked %q", rpc.gotMethod)
	}
	if got != "hi hi" {
		t.Errorf("result %v, want hi hi", got)
	}
}

func TestReflectedClientRejectsAnUnknownMethod(t *testing.T) {
	r := snapshotOf(t)
	client := &ReflectedAgentClient{agentType: r, rpc: &fakeRPC{}}
	if _, err := client.InvokeAndAwait("absent", nil); err == nil ||
		!strings.Contains(err.Error(), "has no method") {
		t.Errorf("error is %v", err)
	}
}

// TestReflectedClientChecksOutputCardinality — the declared shape is part of
// the contract, so a target that returns nothing where a result was promised is
// a remote output error rather than a silent nil.
func TestReflectedClientChecksOutputCardinality(t *testing.T) {
	r := snapshotOf(t)
	client := &ReflectedAgentClient{agentType: r, rpc: &fakeRPC{has: false}}

	_, err := client.InvokeAndAwait("greet", map[string]any{"greeting": "hi", "times": 1})
	if err == nil || !strings.Contains(err.Error(), "declares a result") {
		t.Errorf("error is %v", err)
	}
}

func TestReflectedClientValidatesBeforeSending(t *testing.T) {
	r := snapshotOf(t)
	rpc := &fakeRPC{}
	client := &ReflectedAgentClient{agentType: r, rpc: rpc}

	// times is an s32; a string is not one.
	_, err := client.InvokeAndAwait("greet", map[string]any{"greeting": "hi", "times": "two"})
	if err == nil {
		t.Fatal("an invalid argument reached the target")
	}
	if rpc.gotMethod != "" {
		t.Error("the call was sent despite failing validation")
	}
}

// TestDiscoveryOffTarget — the native build has no deployment to discover, and
// must say so rather than pretend.
func TestDiscoveryOffTarget(t *testing.T) {
	if got := DiscoverAgentTypes(); len(got) != 0 {
		t.Errorf("discovered %d agent types off-target", len(got))
	}
	if _, found := DiscoverAgentType("Greeter"); found {
		t.Error("discovery found an agent type off-target")
	}
	r := snapshotOf(t)
	if _, err := r.Bind(map[string]any{"name": "ada"}); err == nil {
		t.Error("binding succeeded off-target")
	}
}

var _ = witTypes.Unit{}
