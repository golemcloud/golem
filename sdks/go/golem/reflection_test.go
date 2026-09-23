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
	"github.com/golemcloud/golem/sdks/go/golem/schema"
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

// toolSnapshotOf derives a tool the way the host publishes it, so the tool
// reflection tests read against real metadata.
func toolSnapshotOf(t *testing.T) ReflectedTool {
	t.Helper()
	r, d := newToolRegistry(), newDefinitions()
	def := defineToolInto(r, d, "files", ToolSpec{Version: "1.0.0", Summary: "File utilities"})
	declareGroup(r, d, def, []string{"index"}, []CommandOpt{Summary("Manage the index")})

	type AddArgs struct {
		Path    Positional[string]
		Force   Flag
		Retries Opt[int32]
	}
	add := declareCommand[AddArgs, string](r, d, def, []string{"index", "add"}, "add", AddArgs{
		Force:   Flag{Short: 'f'},
		Retries: Opt[int32]{Default: Some(int32(1))},
	}, []CommandOpt{Summary("Add a file"), Aliases("a")})
	handleCommandInto(r, d, add, func(_ *ToolContext, in AddArgs) string { return in.Path.Get() })

	tools, ok := r.discover(d)
	if !ok {
		t.Fatalf("tool discovery failed: %s", allDefErrors(d.errs))
	}
	return ReflectedTool{lookupName: "files", wit: tools[0]}
}

// TestToolSnapshotWalksTheCommandTree — a caller with no Go types for the tool
// navigates it by name, including through namespace nodes and aliases.
func TestToolSnapshotWalksTheCommandTree(t *testing.T) {
	r := toolSnapshotOf(t)
	if r.Name() != "files" || r.Version() != "1.0.0" {
		t.Errorf("snapshot is %q/%q", r.Name(), r.Version())
	}

	// The root and the group dispatch only.
	if r.Root().Callable() {
		t.Error("the root gained a body it never declared")
	}
	group, found := r.Command([]string{"index"})
	if !found || group.Callable() {
		t.Errorf("index is found=%v callable=%v, want found and not callable", found, group.Callable())
	}

	add, found := r.Command([]string{"index", "add"})
	if !found || !add.Callable() {
		t.Fatalf("index add is found=%v callable=%v", found, add.Callable())
	}
	if add.Description() != "Add a file" {
		t.Errorf("description %q", add.Description())
	}
	if got := strings.Join(add.Path(), " "); got != "index add" {
		t.Errorf("path %q", got)
	}

	// Aliases resolve too.
	if _, found := r.Command([]string{"index", "a"}); !found {
		t.Error("the alias did not resolve")
	}
	if _, found := r.Command([]string{"index", "nope"}); found {
		t.Error("an undeclared command resolved")
	}

	// Commands enumerates the whole tree, namespaces included.
	if n := len(r.Commands()); n != 3 {
		t.Errorf("enumerated %d commands, want 3", n)
	}
}

// TestToolArgumentsAreOneParameterList — positionals, options and flags all
// become fields of the single record an invocation carries.
func TestToolArgumentsAreOneParameterList(t *testing.T) {
	r := toolSnapshotOf(t)
	add, _ := r.Command([]string{"index", "add"})

	params, err := add.Arguments()
	if err != nil {
		t.Fatalf("Arguments: %v", err)
	}
	var names []string
	for _, p := range params {
		names = append(names, p.Name)
	}
	if strings.Join(names, ",") != "path,retries,force" {
		t.Errorf("parameters are %v, want positionals then options then flags", names)
	}

	// A flag's type is fixed rather than named by the graph.
	for _, p := range params {
		if p.Name == "force" && p.Node != schema.BoolParameterNode {
			t.Errorf("the flag names graph node %d instead of being a fixed bool", p.Node)
		}
	}
}

func TestToolCommandPacksAndRendersItsArguments(t *testing.T) {
	r := toolSnapshotOf(t)
	add, _ := r.Command([]string{"index", "add"})

	input, err := add.PackJSON(map[string]any{"path": "/tmp/a", "force": true, "retries": 3})
	if err != nil {
		t.Fatalf("PackJSON: %v", err)
	}
	root := input.Value.ValueNodes[input.Value.Root]
	if root.Tag() != types.SchemaValueNodeRecordValue || len(root.RecordValue()) != 3 {
		t.Fatalf("input root is %v", root)
	}

	rendered, err := add.ToJSONSchema(false)
	if err != nil {
		t.Fatalf("ToJSONSchema: %v", err)
	}
	data, _ := json.Marshal(rendered)
	var doc map[string]any
	_ = json.Unmarshal(data, &doc)
	props, _ := doc["properties"].(map[string]any)
	force, _ := props["force"].(map[string]any)
	if force["type"] != "boolean" {
		t.Errorf("the flag rendered as %v, want a boolean", force)
	}
}

// fakeToolRPC replays a scripted invocation result.
type fakeToolRPC struct {
	gotPath []string
	out     types.TypedSchemaValue
	has     bool
	err     error
}

func (f *fakeToolRPC) invokeAndAwait(path []string, _ types.TypedSchemaValue) (types.TypedSchemaValue, bool, error) {
	f.gotPath = path
	return f.out, f.has, f.err
}

func TestReflectedToolClientInvokes(t *testing.T) {
	r := toolSnapshotOf(t)
	result, err := EncodeTypedValue("/tmp/a")
	if err != nil {
		t.Fatalf("EncodeTypedValue: %v", err)
	}
	rpc := &fakeToolRPC{out: result.wit, has: true}
	client := &ReflectedToolClient{tool: r, rpc: rpc}

	got, err := client.InvokeAndAwait([]string{"index", "add"},
		map[string]any{"path": "/tmp/a", "force": false, "retries": 1})
	if err != nil {
		t.Fatalf("InvokeAndAwait: %v", err)
	}
	if strings.Join(rpc.gotPath, " ") != "index add" {
		t.Errorf("invoked %v", rpc.gotPath)
	}
	if got != "/tmp/a" {
		t.Errorf("result %v", got)
	}
}

// TestReflectedToolClientRefusesANamespace — a dispatch-only node is
// discoverable but has nothing to run.
func TestReflectedToolClientRefusesANamespace(t *testing.T) {
	r := toolSnapshotOf(t)
	client := &ReflectedToolClient{tool: r, rpc: &fakeToolRPC{}}

	_, err := client.InvokeAndAwait([]string{"index"}, nil)
	if err == nil || !strings.Contains(err.Error(), "only dispatches to subcommands") {
		t.Errorf("error is %v", err)
	}
}

func TestReflectedToolClientValidatesBeforeSending(t *testing.T) {
	r := toolSnapshotOf(t)
	rpc := &fakeToolRPC{}
	client := &ReflectedToolClient{tool: r, rpc: rpc}

	_, err := client.InvokeAndAwait([]string{"index", "add"},
		map[string]any{"path": "/tmp/a", "force": "yes", "retries": 1})
	if err == nil {
		t.Fatal("an invalid flag reached the target")
	}
	if rpc.gotPath != nil {
		t.Error("the call was sent despite failing validation")
	}
}

// TestDynamicAgentClientInvokesWithPackedValues — a dynamic caller already
// holds schema-native values and keeps no snapshot, so the client neither packs
// nor validates for it.
func TestDynamicAgentClientInvokesWithPackedValues(t *testing.T) {
	r := snapshotOf(t)
	m, _ := r.Method("greet")
	input, err := m.PackJSON(map[string]any{"greeting": "hi", "times": 1})
	if err != nil {
		t.Fatalf("PackJSON: %v", err)
	}
	out, _ := m.Output()
	result, err := out.PackJSON("hi")
	if err != nil {
		t.Fatalf("packing the result: %v", err)
	}

	rpc := &fakeRPC{tree: result, has: true}
	client := &DynamicAgentClient{agentID: "greeter-1", rpc: rpc}

	got, err := client.InvokeDynamic("greet", input)
	if err != nil {
		t.Fatalf("InvokeDynamic: %v", err)
	}
	if rpc.gotMethod != "greet" {
		t.Errorf("invoked %q", rpc.gotMethod)
	}
	if got.IsNone() {
		t.Fatal("the result was dropped")
	}
	value, err := out.UnpackJSON(got.Unwrap())
	if err != nil || value != "hi" {
		t.Errorf("result %v (%v)", value, err)
	}
}

// TestDynamicAgentClientInvokeJSON — the caller may also hand over a schema of
// its own choosing rather than packing by hand.
func TestDynamicAgentClientInvokeJSON(t *testing.T) {
	r := snapshotOf(t)
	m, _ := r.Method("greet")
	out, _ := m.Output()
	result, _ := out.PackJSON("hi")

	rpc := &fakeRPC{tree: result, has: true}
	client := &DynamicAgentClient{agentID: "greeter-1", rpc: rpc}

	got, err := client.InvokeJSON("greet", r.Schema(), m.Parameters(),
		map[string]any{"greeting": "hi", "times": 1})
	if err != nil {
		t.Fatalf("InvokeJSON: %v", err)
	}
	if got.IsNone() {
		t.Fatal("the result was dropped")
	}
}

// TestInvokeUsesTheCallersOwnTypes — a caller-defined client owns its
// compile-time types and only borrows the identity.
func TestInvokeUsesTheCallersOwnTypes(t *testing.T) {
	r := snapshotOf(t)
	m, _ := r.Method("greet")
	out, _ := m.Output()
	result, _ := out.PackJSON("hi hi")

	rpc := &fakeRPC{tree: result, has: true}
	client := &DynamicAgentClient{agentID: "greeter-1", rpc: rpc}

	got, err := Invoke[GreetIn, string](client, "greet", GreetIn{Greeting: "hi", Times: 2})
	if err != nil {
		t.Fatalf("Invoke: %v", err)
	}
	if got != "hi hi" {
		t.Errorf("result %q, want hi hi", got)
	}
}

// TestInvokeChecksOutputCardinality — the caller's declared Out is the only
// contract here, so a mismatch has to be reported rather than zero-valued.
func TestInvokeChecksOutputCardinality(t *testing.T) {
	client := &DynamicAgentClient{agentID: "greeter-1", rpc: &fakeRPC{has: false}}
	if _, err := Invoke[GreetIn, string](client, "greet", GreetIn{}); err == nil ||
		!strings.Contains(err.Error(), "returned nothing") {
		t.Errorf("error is %v", err)
	}

	r := snapshotOf(t)
	m, _ := r.Method("greet")
	out, _ := m.Output()
	result, _ := out.PackJSON("hi")
	client = &DynamicAgentClient{agentID: "greeter-1", rpc: &fakeRPC{tree: result, has: true}}
	if _, err := Invoke[GreetIn, Unit](client, "greet", GreetIn{}); err == nil ||
		!strings.Contains(err.Error(), "golem.Unit") {
		t.Errorf("error is %v", err)
	}
}

func TestDynamicToolClientInvokes(t *testing.T) {
	r := toolSnapshotOf(t)
	add, _ := r.Command([]string{"index", "add"})
	input, err := add.PackJSON(map[string]any{"path": "/tmp/a", "force": false, "retries": 1})
	if err != nil {
		t.Fatalf("PackJSON: %v", err)
	}
	result, _ := EncodeTypedValue("/tmp/a")

	rpc := &fakeToolRPC{out: result.wit, has: true}
	client := &DynamicToolClient{toolName: "files", rpc: rpc}

	got, err := client.InvokeDynamic([]string{"index", "add"}, TypedValue{wit: input})
	if err != nil {
		t.Fatalf("InvokeDynamic: %v", err)
	}
	if strings.Join(rpc.gotPath, " ") != "index add" {
		t.Errorf("invoked %v", rpc.gotPath)
	}
	value, err := got.Unwrap().JSON()
	if err != nil || value != "/tmp/a" {
		t.Errorf("result %v (%v)", value, err)
	}
}

func TestBindingOffTarget(t *testing.T) {
	if _, err := BindAgentID("anything"); err == nil {
		t.Error("binding an agent id succeeded off-target")
	}
	if _, err := BindTool("files"); err == nil {
		t.Error("binding a tool succeeded off-target")
	}
	if _, err := ParseRawAgentID("anything"); err == nil {
		t.Error("parsing an agent id succeeded off-target")
	}
}
