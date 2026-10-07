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
	"errors"
	"io"
	"reflect"
	"strings"
	"testing"
	"time"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	"github.com/golemcloud/golem/sdks/go/golem/internal/witschema"
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
	return newReflectedAgentType(found[0])
}

// TestSnapshotDescribesTheAgentType — the snapshot is everything a caller gets;
// it has none of the target's Go types.
func TestSnapshotDescribesTheAgentType(t *testing.T) {
	r := snapshotOf(t)
	if r.Name() != "Greeter" || r.Description() != "Greets people" || r.Mode() != Durable {
		t.Errorf("snapshot is %q/%q/%v", r.Name(), r.Description(), r.Mode())
	}

	ctor := need(r.Constructor().Input())
	if got := recordFieldNames(ctor); strings.Join(got, ",") != "name" {
		t.Fatalf("constructor input fields are %v, want [name]", got)
	}

	m, known := r.Method("greet")
	if !known {
		t.Fatal("the snapshot has no greet method")
	}
	if m.Description() != "Greet someone" {
		t.Errorf("method description %q", m.Description())
	}
	if got := recordFieldNames(need(m.Input())); strings.Join(got, ",") != "greeting,times" {
		t.Fatalf("method input fields are %v", got)
	}
	if need(m.Output()).IsNone() {
		t.Error("greet declares no output")
	}
	if _, known := r.Method("absent"); known {
		t.Error("an undeclared method was found")
	}
}

// need unwraps a (value, error) pair, failing the test through a panic.
func need[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

func recordFieldNames(ref core.Ref) []string {
	record, _ := ref.Type().Body.(core.RecordType)
	out := make([]string, 0, len(record.Fields))
	for _, f := range record.Fields {
		out = append(out, f.Name)
	}
	return out
}

// TestInputPacksTheInvocationRecord — a reflective call is named arguments
// packed against the snapshot's input record, in the parameter list's order.
func TestInputPacksTheInvocationRecord(t *testing.T) {
	r := snapshotOf(t)
	m, _ := r.Method("greet")

	packed, err := need(m.Input()).PackJSON(map[string]any{"greeting": "hi", "times": 2})
	if err != nil {
		t.Fatalf("PackJSON: %v", err)
	}
	rec, ok := packed.(core.RecordValue)
	if !ok || len(rec.Fields) != 2 {
		t.Fatalf("packed %#v, want a record of 2 fields", packed)
	}

	// On the wire, the same value decodes back into the agent's own Go type.
	tree, err := witschema.ValueToWit(packed)
	if err != nil {
		t.Fatal(err)
	}
	var in GreetIn
	fields := newDefinitions().StructFields(reflect.TypeFor[GreetIn]())
	if err := decodeParams(tree, fields, reflect.ValueOf(&in).Elem(), nil); err != nil {
		t.Fatalf("the packed tree is not readable by the target: %v", err)
	}
	if in.Greeting != "hi" || in.Times != 2 {
		t.Errorf("decoded %+v", in)
	}

	back, err := need(m.Input()).UnpackJSON(packed)
	if err != nil {
		t.Fatal(err)
	}
	if obj := back.(map[string]any); obj["greeting"] != "hi" {
		t.Errorf("round trip gave %v", back)
	}
}

// TestInputReportsEveryProblemAtOnce — a caller working from JSON should learn
// about all its mistakes in one go.
func TestInputReportsEveryProblemAtOnce(t *testing.T) {
	r := snapshotOf(t)
	m, _ := r.Method("greet")
	_, err := need(m.Input()).PackJSON(map[string]any{"greetng": "hi", "extra": 1})
	if err == nil {
		t.Fatal("packing malformed arguments succeeded")
	}
	for _, want := range []string{"greetng", "extra", "greeting", "times"} {
		if !strings.Contains(err.Error(), want) {
			t.Errorf("message does not mention %q: %s", want, err)
		}
	}
}

// TestInputJSONSchemaDescribesItsArguments — this is what a model is handed to
// fill in, so it must name every parameter and mark the required ones.
func TestInputJSONSchemaDescribesItsArguments(t *testing.T) {
	r := snapshotOf(t)
	m, _ := r.Method("greet")
	rendered, err := need(m.Input()).ToJSONSchema(true)
	if err != nil {
		t.Fatalf("ToJSONSchema: %v", err)
	}
	data, _ := json.Marshal(rendered)
	var doc map[string]any
	_ = json.Unmarshal(data, &doc)
	if doc["type"] != "object" {
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
}

// TestAutoInjectedFieldsAreNotAskedOfTheCaller — the host fills the principal
// in, so a caller can neither know it nor override it.
func TestAutoInjectedFieldsAreNotAskedOfTheCaller(t *testing.T) {
	in := common.MakeInputSchemaParameters([]common.NamedField{
		{Name: "name", Source: common.MakeFieldSourceUserSupplied(), Schema: 0},
		{Name: "principal", Source: common.MakeFieldSourceAutoInjected(common.AutoInjectedKindPrincipal), Schema: 0},
	})
	conv, err := witschema.GraphToCore(types.SchemaGraph{
		TypeNodes: []types.SchemaTypeNode{{Body: types.MakeSchemaTypeBodyStringType()}},
		Root:      0,
	})
	if err != nil {
		t.Fatalf("GraphToCore: %v", err)
	}
	ref := need(parametersRecord(conv, nil, in))
	if got := recordFieldNames(ref); strings.Join(got, ",") != "name" {
		t.Errorf("fields are %v, want only the caller-supplied one", got)
	}
}

// fakeRPC replays a scripted invocation result and records what was sent.
type fakeRPC struct {
	gotMethod string
	gotForm   string
	tree      types.SchemaValueTree
	has       bool
	err       error
	cancelled bool
}

var fakeID = InvocationID{AgentID: "Greeter(\"ada\")", IdempotencyKey: "k1"}

func (f *fakeRPC) start(method string, _ types.SchemaValueTree) (pendingRPC, error) {
	f.gotMethod, f.gotForm = method, "call"
	return pendingRPC{
		id: fakeID,
		wait: func() (witTypes.Option[types.SchemaValueTree], error) {
			if !f.has {
				return witTypes.None[types.SchemaValueTree](), f.err
			}
			return witTypes.Some(f.tree), f.err
		},
		cancel: func() { f.cancelled = true },
	}, nil
}

func (f *fakeRPC) trigger(method string, _ types.SchemaValueTree) (InvocationID, error) {
	f.gotMethod, f.gotForm = method, "trigger"
	return fakeID, f.err
}

func (f *fakeRPC) schedule(_ time.Time, method string, _ types.SchemaValueTree) (*ScheduledInvocation, error) {
	f.gotMethod, f.gotForm = method, "schedule"
	return &ScheduledInvocation{ID: fakeID}, f.err
}

func greetResult(t *testing.T, r ReflectedAgentType, value string) types.SchemaValueTree {
	t.Helper()
	m, _ := r.Method("greet")
	result, err := packWit(need(m.Output()).Unwrap(), value)
	if err != nil {
		t.Fatalf("packing the result: %v", err)
	}
	return result
}

func TestReflectedClientCallsAndDecodes(t *testing.T) {
	r := snapshotOf(t)
	rpc := &fakeRPC{tree: greetResult(t, r, "hi hi"), has: true}
	client := &ReflectedAgentClient{agentType: r, agentID: "greeter-1", rpc: rpc}

	got, id, err := client.Call("greet", map[string]any{"greeting": "hi", "times": 2})
	if err != nil {
		t.Fatalf("Call: %v", err)
	}
	if rpc.gotMethod != "greet" || got != "hi hi" || id != fakeID {
		t.Errorf("invoked %q, got %v, id %+v", rpc.gotMethod, got, id)
	}

	id, err = client.Trigger("greet", map[string]any{"greeting": "hi", "times": 1})
	if err != nil || rpc.gotForm != "trigger" || id != fakeID {
		t.Errorf("Trigger: %v %q %+v", err, rpc.gotForm, id)
	}
	sched, err := client.Schedule(time.Now(), "greet", map[string]any{"greeting": "hi", "times": 1})
	if err != nil || rpc.gotForm != "schedule" || sched.ID != fakeID {
		t.Errorf("Schedule: %v %q %+v", err, rpc.gotForm, sched)
	}
}

func TestReflectedClientRejectsAnUnknownMethod(t *testing.T) {
	r := snapshotOf(t)
	client := &ReflectedAgentClient{agentType: r, rpc: &fakeRPC{}}
	if _, _, err := client.Call("absent", nil); err == nil || !strings.Contains(err.Error(), "has no method") {
		t.Errorf("error is %v", err)
	}
}

// TestReflectedClientChecksOutputCardinality — the declared shape is part of
// the contract, so a target that returns nothing where a result was promised is
// a remote output error rather than a silent nil.
func TestReflectedClientChecksOutputCardinality(t *testing.T) {
	r := snapshotOf(t)
	client := &ReflectedAgentClient{agentType: r, rpc: &fakeRPC{has: false}}
	_, _, err := client.Call("greet", map[string]any{"greeting": "hi", "times": 1})
	if err == nil || !strings.Contains(err.Error(), "declares a result") {
		t.Errorf("error is %v", err)
	}
}

func TestReflectedClientValidatesBeforeSending(t *testing.T) {
	r := snapshotOf(t)
	rpc := &fakeRPC{}
	client := &ReflectedAgentClient{agentType: r, rpc: rpc}
	// times is an s32; a string is not one.
	if _, _, err := client.Call("greet", map[string]any{"greeting": "hi", "times": "two"}); err == nil {
		t.Fatal("an invalid argument reached the target")
	}
	if _, err := client.Trigger("greet", map[string]any{"greeting": 1}); err == nil {
		t.Fatal("an invalid triggered argument reached the target")
	}
	if rpc.gotMethod != "" {
		t.Error("a call was sent despite failing validation")
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
	if _, err := r.Get(map[string]any{"name": "ada"}); err == nil {
		t.Error("Get succeeded off-target")
	}
	if _, err := r.Bind("Greeter(\"ada\")"); err == nil {
		t.Error("Bind succeeded off-target")
	}
}

var _ = witTypes.Unit{}

// toolSnapshotOf derives a tool the way the host publishes it, so the tool
// reflection tests read against real metadata.
func toolSnapshotOf(t *testing.T) ReflectedTool {
	t.Helper()
	r, d := newToolRegistry(), newDefinitions()
	def := defineToolInto[Files](r, d, "files", ToolSpec{Version: "1.0.0", Summary: "File utilities"}, false)
	index := def.Group("index").Doc("Manage the index")

	type AddArgs struct {
		Path    string
		Force   bool
		Retries int32
	}
	add := index.Command[AddArgs, string]("add", func(a *AddArgs, s *ToolCommandSpec) {
		s.Doc("Add a file")
		s.Aliases("a")
		s.Positional(&a.Path)
		s.Flag(&a.Force).Short('f')
		s.Option(&a.Retries).Default(1)
	})
	_ = add.Handle(func(_ *ToolContext, in AddArgs) (string, error) { return in.Path, nil })

	tools, ok := r.discover(d)
	if !ok {
		t.Fatalf("tool discovery failed: %s", allDefErrors(d.Errs))
	}
	return newReflectedTool("files", tools[0])
}

type Files struct{}

// TestToolSnapshotWalksTheCommandTree — a caller with no Go types for the tool
// navigates it by name, including through namespace nodes and aliases.
func TestToolSnapshotWalksTheCommandTree(t *testing.T) {
	r := toolSnapshotOf(t)
	if r.Name() != "files" || r.Version() != "1.0.0" {
		t.Errorf("snapshot is %q/%q", r.Name(), r.Version())
	}
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
	if add.Description() != "Add a file" || strings.Join(add.Path(), " ") != "index add" {
		t.Errorf("description %q path %v", add.Description(), add.Path())
	}
	if _, found := r.Command([]string{"index", "a"}); !found {
		t.Error("the alias did not resolve")
	}
	if _, found := r.Command([]string{"index", "nope"}); found {
		t.Error("an undeclared command resolved")
	}
	if n := len(r.Commands()); n != 3 {
		t.Errorf("enumerated %d commands, want 3", n)
	}
}

// TestToolInputIsTheCanonicalRecord — positionals, options and flags all
// become fields of the single record an invocation carries.
func TestToolInputIsTheCanonicalRecord(t *testing.T) {
	r := toolSnapshotOf(t)
	add, _ := r.Command([]string{"index", "add"})
	input := need(add.Input())
	if got := recordFieldNames(input); strings.Join(got, ",") != "path,retries,force" {
		t.Errorf("fields are %v, want positionals then options then flags", got)
	}
	record := input.Type().Body.(core.RecordType)
	if _, isBool := record.Fields[2].Body.Body.(core.BoolType); !isBool {
		t.Errorf("the flag is typed %T instead of a bool", record.Fields[2].Body.Body)
	}
	if need(add.Output()).IsNone() {
		t.Error("add declares no output")
	}

	rendered, err := input.ToJSONSchema(false)
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

// recordToolCalls routes tool calls to a scripted outcome and records them.
func recordToolCalls(t *testing.T, outcome func(path []string) (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError)) *[][]string {
	t.Helper()
	var calls [][]string
	prev := startToolCall
	t.Cleanup(func() { startToolCall = prev })
	startToolCall = func(_ string, path []string, input types.TypedSchemaValue, _ io.Reader, _ ToolStreams) (toolCall, error) {
		root := input.Graph.TypeNodes[input.Graph.Root].Body
		if root.Tag() != types.SchemaTypeBodyRecordType {
			t.Errorf("the input graph is not rooted at a record")
		}
		calls = append(calls, path)
		return toolCall{wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
			return outcome(path)
		}, cancel: func() {}}, nil
	}
	return &calls
}

func TestReflectedToolClientCalls(t *testing.T) {
	r := toolSnapshotOf(t)
	result, _ := EncodeTypedValue("/tmp/a")
	calls := recordToolCalls(t, func([]string) (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
		return witTypes.Some(result.wit), nil
	})
	client := need(r.Bind())

	got, err := client.Call([]string{"index", "add"}, map[string]any{"path": "/tmp/a", "force": false, "retries": 1})
	if err != nil {
		t.Fatalf("Call: %v", err)
	}
	if len(*calls) != 1 || strings.Join((*calls)[0], " ") != "index add" || got != "/tmp/a" {
		t.Errorf("calls %v result %v", *calls, got)
	}
}

// TestReflectedToolClientKeepsDeclaredErrorsStructured — a declared tool error
// reaches the caller with its name and payload rather than as a message.
func TestReflectedToolClientKeepsDeclaredErrorsStructured(t *testing.T) {
	r := toolSnapshotOf(t)
	payload, _ := EncodeTypedValue("missing.txt")
	recordToolCalls(t, func([]string) (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
		e := types.MakeToolRpcErrorRemoteToolError(types.MakeToolErrorCustomError(types.CustomToolError{
			Name: "not-found", Payload: payload.wit,
		}))
		return witTypes.None[types.TypedSchemaValue](), &e
	})
	client := need(r.Bind())
	_, err := client.Call([]string{"index", "add"}, map[string]any{"path": "/x", "force": false, "retries": 1})
	var ce *ToolCallError
	if !errors.As(err, &ce) || ce.Kind != ToolCallDeclaredError || ce.ErrorName != "not-found" {
		t.Fatalf("got %v", err)
	}
	if v, err := ce.Payload().JSON(); err != nil || v != "missing.txt" {
		t.Errorf("payload %v (%v)", v, err)
	}
}

// TestReflectedToolClientRefusesANamespace — a dispatch-only node is
// discoverable but has nothing to run.
func TestReflectedToolClientRefusesANamespace(t *testing.T) {
	r := toolSnapshotOf(t)
	client := need(r.Bind())
	_, err := client.Call([]string{"index"}, nil)
	if err == nil || !strings.Contains(err.Error(), "only dispatches to subcommands") {
		t.Errorf("error is %v", err)
	}
}

func TestReflectedToolClientValidatesBeforeSending(t *testing.T) {
	r := toolSnapshotOf(t)
	calls := recordToolCalls(t, func([]string) (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
		return witTypes.None[types.TypedSchemaValue](), nil
	})
	client := need(r.Bind())
	_, err := client.Call([]string{"index", "add"}, map[string]any{"path": "/tmp/a", "force": "yes", "retries": 1})
	if err == nil {
		t.Fatal("an invalid flag reached the target")
	}
	if len(*calls) != 0 {
		t.Error("the call was sent despite failing validation")
	}
}

// TestDynamicAgentClientCallsWithPackedValues — a dynamic caller already holds
// schema-native values and keeps no snapshot, so the client neither packs nor
// validates for it; a discovered snapshot can do both around the call.
func TestDynamicAgentClientCallsWithPackedValues(t *testing.T) {
	r := snapshotOf(t)
	m, _ := r.Method("greet")
	input, err := need(m.Input()).PackJSON(map[string]any{"greeting": "hi", "times": 1})
	if err != nil {
		t.Fatalf("PackJSON: %v", err)
	}
	rpc := &fakeRPC{tree: greetResult(t, r, "hi"), has: true}
	client := &DynamicAgentClient{agentID: "greeter-1", rpc: rpc}

	got, id, err := client.Call("greet", input)
	if err != nil {
		t.Fatalf("Call: %v", err)
	}
	if rpc.gotMethod != "greet" || id != fakeID || got.IsNone() {
		t.Fatalf("invoked %q id %+v result %v", rpc.gotMethod, id, got)
	}
	value, err := need(m.Output()).Unwrap().UnpackJSON(got.Unwrap())
	if err != nil || value != "hi" {
		t.Errorf("result %v (%v)", value, err)
	}
	if _, err := client.Trigger("greet", input); err != nil || rpc.gotForm != "trigger" {
		t.Errorf("Trigger: %v %q", err, rpc.gotForm)
	}
}

func TestDynamicToolClientCalls(t *testing.T) {
	r := toolSnapshotOf(t)
	add, _ := r.Command([]string{"index", "add"})
	input, err := add.pack(map[string]any{"path": "/tmp/a", "force": false, "retries": 1})
	if err != nil {
		t.Fatalf("pack: %v", err)
	}
	result, _ := EncodeTypedValue("/tmp/a")
	calls := recordToolCalls(t, func([]string) (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
		return witTypes.Some(result.wit), nil
	})
	client := need(BindTool("files"))
	got, err := client.Call([]string{"index", "add"}, TypedValue{wit: input})
	if err != nil {
		t.Fatalf("Call: %v", err)
	}
	if len(*calls) != 1 {
		t.Errorf("calls %v", *calls)
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
	if _, err := ParseRawAgentID("anything"); err == nil {
		t.Error("parsing an agent id succeeded off-target")
	}
	if _, err := MakeAgentID("Greeter", TypedValue{}, None[UUID]()); err == nil {
		t.Error("making an agent id succeeded off-target")
	}
}

// packWit packs canonical JSON and flattens it the way the wire carries it,
// which is what the fake transports expect.
func packWit(ref core.Ref, value any) (types.SchemaValueTree, error) {
	built, err := ref.PackJSON(value)
	if err != nil {
		return types.SchemaValueTree{}, err
	}
	return witschema.ValueToWit(built)
}

// TestReflectedConfigOverridesAreChecked — untyped configuration entries are
// checked against the snapshot's declarations, every problem at once, and
// rendered as the declared type.
func TestReflectedConfigOverridesAreChecked(t *testing.T) {
	d := newDefinitions()
	cfgConfiguredAgent[demoAppConfig](d)
	found, errs := d.discover()
	if len(errs) > 0 {
		t.Fatalf("definition errors: %s", allDefErrors(errs))
	}
	r := newReflectedAgentType(found[0])

	values, err := r.configValues([]configOverride{
		{path: []string{"greeting"}, json: "hello"},
		{path: []string{"db", "url"}, value: core.StringValue{Value: "pg://db"}, native: true},
	})
	if err != nil || len(values) != 2 {
		t.Fatalf("valid overrides gave %d values, %v", len(values), err)
	}
	for i, want := range []string{"hello", "pg://db"} {
		v, err := witschema.ValueToCore(values[i].Value.Value)
		if err != nil || v.(core.StringValue).Value != want {
			t.Errorf("override %d is %v, %v", i, v, err)
		}
		if root := values[i].Value.Graph.TypeNodes[values[i].Value.Graph.Root].Body.Tag(); root != types.SchemaTypeBodyStringType {
			t.Errorf("override %d is typed as tag %d", i, root)
		}
	}

	_, err = r.configValues([]configOverride{
		{path: []string{"greting"}, json: "typo"},
		{path: []string{"db", "password"}, json: "hunter2"},
		{path: []string{"greeting"}, json: 42},
		{path: []string{"db", "url"}, value: core.BoolValue{Value: true}, native: true},
	})
	for _, want := range []string{
		"greting is not a declared configuration path",
		"db.password is a secret",
		"greeting: ",
		"db.url: ",
	} {
		if err == nil || !strings.Contains(err.Error(), want) {
			t.Errorf("invalid overrides gave %v, want it to mention %q", err, want)
		}
	}
}

// TestAPendingCallIsCancelledOrAwaited — a pending call identifies itself
// before its result, and cancelling it stops the wait.
func TestAPendingCallIsCancelledOrAwaited(t *testing.T) {
	r := snapshotOf(t)
	rpc := &fakeRPC{tree: greetResult(t, r, "hi"), has: true}
	client := &ReflectedAgentClient{agentType: r, agentID: "greeter-1", rpc: rpc}

	p, err := client.CallAsync("greet", map[string]any{"greeting": "hi", "times": 1})
	if err != nil || p.ID != fakeID {
		t.Fatalf("CallAsync gave %+v, %v", p, err)
	}
	if got, err := p.Wait(); got != "hi" || err != nil {
		t.Fatalf("Wait gave %v, %v", got, err)
	}
	p.Cancel()
	if rpc.cancelled {
		t.Error("cancelling an awaited call reached the host")
	}

	p, _ = client.CallAsync("greet", map[string]any{"greeting": "hi", "times": 1})
	p.Cancel()
	if _, err := p.Wait(); !errors.Is(err, ErrCallCancelled) || !rpc.cancelled {
		t.Errorf("a cancelled call gave %v (cancelled=%v)", err, rpc.cancelled)
	}
}
