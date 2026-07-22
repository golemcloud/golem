package golem

import (
	"strings"
	"testing"

	guestExports "github.com/golemcloud/golem-go/internal/exports/export_golem_agent_guest"
	common "github.com/golemcloud/golem-go/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem-go/internal/wit/golem_core_types"
)

// These tests run natively (host arch): they exercise the real export slots and
// the real dispatch path. That works because the generated bindings compile for
// the host; only *calling* a host import would be a link error.

type tCounterId struct{ Name string }
type tCounterState struct{ count int64 }
type tAddIn struct{ By int64 }

var tCounter = DefineAgent[tCounterId, tCounterState](
	Spec{Name: "TestCounter", Description: "counter under test", Mode: Durable},
	func(id tCounterId) *tCounterState { return &tCounterState{} },
)

var (
	tValue = DefineMethod[tCounterId, Unit, int64]("value", Desc("current value"))
	tInc   = DefineMethod[tCounterId, Unit, int64]("increment")
	tAdd   = DefineMethod[tCounterId, tAddIn, int64]("add")
	tReset = DefineMethod[tCounterId, Unit, Unit]("reset")
	tBoom  = DefineMethod[tCounterId, Unit, int64]("boom")
)

func init() {
	Implement(tCounter, tValue, func(ctx *Context[tCounterState], _ Unit) (int64, error) {
		return ctx.State.count, nil
	})
	Implement(tCounter, tInc, func(ctx *Context[tCounterState], _ Unit) (int64, error) {
		ctx.State.count++
		return ctx.State.count, nil
	})
	Implement(tCounter, tAdd, func(ctx *Context[tCounterState], in tAddIn) (int64, error) {
		ctx.State.count += in.By
		return ctx.State.count, nil
	})
	Implement(tCounter, tReset, Bind0Unit((*tCounterState).reset)) // method-expression binding
	Implement(tCounter, tBoom, func(*Context[tCounterState], Unit) (int64, error) {
		panic("kaboom from agent code")
	})
}

func (s *tCounterState) reset() { s.count = 0 }

// params builds a parameter-list tree: a record whose fields are the values.
func params(vals ...types.SchemaValueNode) types.SchemaValueTree {
	nodes := make([]types.SchemaValueNode, 0, len(vals)+1)
	idxs := make([]int32, 0, len(vals))
	for _, v := range vals {
		idxs = append(idxs, int32(len(nodes)))
		nodes = append(nodes, v)
	}
	root := int32(len(nodes))
	nodes = append(nodes, types.MakeSchemaValueNodeRecordValue(idxs))
	return types.SchemaValueTree{ValueNodes: nodes, Root: root}
}

func initAgent(t *testing.T) {
	t.Helper()
	active = nil // fresh instance per test
	res := guestExports.Initialize("TestCounter", params(types.MakeSchemaValueNodeStringValue("x")), common.MakePrincipalAnonymous())
	if res.IsErr() {
		t.Fatalf("initialize failed: %v", res.Err())
	}
}

func invokeInt(t *testing.T, method string, in types.SchemaValueTree) int64 {
	t.Helper()
	res := guestExports.Invoke(method, in, common.MakePrincipalAnonymous())
	if res.IsErr() {
		t.Fatalf("%s errored: %v", method, res.Err())
	}
	opt := res.Ok()
	if opt.IsNone() {
		t.Fatalf("%s returned none, expected a value", method)
	}
	tree := opt.Some()
	return tree.ValueNodes[tree.Root].S64Value()
}

func TestCounterRoundTrip(t *testing.T) {
	initAgent(t)
	if got := invokeInt(t, "increment", params()); got != 1 {
		t.Fatalf("increment = %d, want 1", got)
	}
	if got := invokeInt(t, "add", params(types.MakeSchemaValueNodeS64Value(5))); got != 6 {
		t.Fatalf("add(5) = %d, want 6", got)
	}
	if got := invokeInt(t, "value", params()); got != 6 {
		t.Fatalf("value = %d, want 6", got)
	}
	// reset returns unit => ok(none); bound via a method expression
	res := guestExports.Invoke("reset", params(), common.MakePrincipalAnonymous())
	if res.IsErr() || !res.Ok().IsNone() {
		t.Fatal("reset should succeed and return none (unit output)")
	}
	if got := invokeInt(t, "value", params()); got != 0 {
		t.Fatalf("value after reset = %d, want 0", got)
	}
}

func TestGetDefinitionDerivesSchemaFromGoTypes(t *testing.T) {
	initAgent(t)
	def := guestExports.GetDefinition()
	if def.TypeName != "TestCounter" || def.SourceLanguage != "go" {
		t.Fatalf("type name = %q, source = %q", def.TypeName, def.SourceLanguage)
	}
	if len(def.Methods) != 5 {
		t.Fatalf("methods = %d, want 5", len(def.Methods))
	}
	// constructor: one parameter, named after the Id field, typed string
	ctor := def.Constructor.InputSchema.Parameters()
	if len(ctor) != 1 || ctor[0].Name != "name" {
		t.Fatalf("constructor params = %+v", ctor)
	}
	if body := def.Schema.TypeNodes[ctor[0].Schema].Body; body.Tag() != types.SchemaTypeBodyStringType {
		t.Fatalf("constructor param type tag = %d, want string", body.Tag())
	}
	// add: one s64 parameter, s64 output
	var add *common.AgentMethod
	for i := range def.Methods {
		if def.Methods[i].Name == "add" {
			add = &def.Methods[i]
		}
	}
	if add == nil {
		t.Fatal("method add missing")
	}
	if add.Description != "" && add.Description != "add" {
		_ = add.Description // description is optional here
	}
	in := add.InputSchema.Parameters()
	if len(in) != 1 || in[0].Name != "by" {
		t.Fatalf("add params = %+v", in)
	}
	if tag := def.Schema.TypeNodes[in[0].Schema].Body.Tag(); tag != types.SchemaTypeBodyS64Type {
		t.Fatalf("add param type tag = %d, want s64", tag)
	}
	if add.OutputSchema.Tag() != common.OutputSchemaSingle {
		t.Fatal("add should have a single output")
	}
	// reset: unit output
	for i := range def.Methods {
		if def.Methods[i].Name == "reset" && def.Methods[i].OutputSchema.Tag() != common.OutputSchemaUnit {
			t.Fatal("reset should have a unit output")
		}
	}
}

func TestDiscoverAgentTypes(t *testing.T) {
	res := guestExports.DiscoverAgentTypes()
	if res.IsErr() {
		t.Fatalf("discover errored: %v", res.Err())
	}
	found := false
	for _, at := range res.Ok() {
		if at.TypeName == "TestCounter" {
			found = true
		}
	}
	if !found {
		t.Fatal("TestCounter not discovered")
	}
}

func TestPanicBecomesCustomErrorAndAgentSurvives(t *testing.T) {
	initAgent(t)
	res := guestExports.Invoke("boom", params(), common.MakePrincipalAnonymous())
	if !res.IsErr() {
		t.Fatal("a panicking method must surface as an agent-error")
	}
	if tag := res.Err().Tag(); tag != common.AgentErrorCustomError {
		t.Fatalf("panic mapped to tag %d, want custom-error", tag)
	}
	// the component must still be usable afterwards
	if got := invokeInt(t, "value", params()); got != 0 {
		t.Fatalf("agent unusable after panic; value = %d", got)
	}
}

func TestUnknownMethodAndBadInputAreDistinguished(t *testing.T) {
	initAgent(t)
	res := guestExports.Invoke("nope", params(), common.MakePrincipalAnonymous())
	if !res.IsErr() || res.Err().Tag() != common.AgentErrorInvalidMethod {
		t.Fatal("unknown method should map to invalid-method")
	}
	// add expects an s64; hand it a string instead
	bad := guestExports.Invoke("add", params(types.MakeSchemaValueNodeStringValue("nope")), common.MakePrincipalAnonymous())
	if !bad.IsErr() || bad.Err().Tag() != common.AgentErrorInvalidInput {
		t.Fatalf("malformed input should map to invalid-input, got tag %d", bad.Err().Tag())
	}
}

func TestPanicErrorAttribution(t *testing.T) {
	agentSide := &PanicError{Method: "m", Stage: stageHandler, Value: "boom"}
	if agentSide.Internal() {
		t.Fatal("a handler panic is not an SDK bug")
	}
	if !strings.Contains(agentSide.Error(), `agent method "m" panicked`) {
		t.Fatalf("unexpected message: %s", agentSide)
	}
	sdkSide := &PanicError{Method: "m", Stage: stageEncode, Value: "boom"}
	if !sdkSide.Internal() || !strings.Contains(sdkSide.Error(), "INTERNAL SDK ERROR") {
		t.Fatalf("unexpected message: %s", sdkSide)
	}
}

// A component may define several agent types; a worker is initialized as exactly
// one of them, and invoke must route into that one's methods only.

type tEchoId struct{ Prefix string }
type tEchoState struct{ prefix string }
type tEchoIn struct{ Msg string }

var tEcho = DefineAgent[tEchoId, tEchoState](
	Spec{Name: "TestEcho", Mode: Ephemeral},
	func(id tEchoId) *tEchoState { return &tEchoState{prefix: id.Prefix} },
)

var tSay = DefineMethod[tEchoId, tEchoIn, string]("say")

func init() {
	Implement(tEcho, tSay, func(ctx *Context[tEchoState], in tEchoIn) (string, error) {
		return ctx.State.prefix + in.Msg, nil
	})
}

func TestWorkerRunsOneOfSeveralAgentTypes(t *testing.T) {
	active = nil
	res := guestExports.Initialize("TestEcho", params(types.MakeSchemaValueNodeStringValue("> ")),
		common.MakePrincipalAnonymous())
	if res.IsErr() {
		t.Fatalf("initialize failed: %v", res.Err())
	}
	// constructor params reached the state
	out := guestExports.Invoke("say", params(types.MakeSchemaValueNodeStringValue("hi")), common.MakePrincipalAnonymous())
	if out.IsErr() {
		t.Fatalf("say errored: %v", out.Err())
	}
	tree := out.Ok().Some()
	if got := tree.ValueNodes[tree.Root].StringValue(); got != "> hi" {
		t.Fatalf("say = %q, want %q", got, "> hi")
	}
	// the other agent's methods are not reachable from this worker
	if r := guestExports.Invoke("increment", params(), common.MakePrincipalAnonymous()); !r.IsErr() {
		t.Fatal("TestCounter's method must not be invocable on a TestEcho worker")
	}
	// get-definition reports the agent this worker was initialized as
	if def := guestExports.GetDefinition(); def.TypeName != "TestEcho" || def.Mode != common.AgentModeEphemeral {
		t.Fatalf("definition = %q mode %d, want TestEcho/ephemeral", def.TypeName, def.Mode)
	}
	// initializing twice is refused
	if again := guestExports.Initialize("TestEcho", params(types.MakeSchemaValueNodeStringValue("x")),
		common.MakePrincipalAnonymous()); !again.IsErr() {
		t.Fatal("re-initializing an already-initialized worker must fail")
	}
	// unknown agent type
	active = nil
	if bad := guestExports.Initialize("NoSuchAgent", params(), common.MakePrincipalAnonymous()); !bad.IsErr() ||
		bad.Err().Tag() != common.AgentErrorInvalidType {
		t.Fatal("unknown agent type should map to invalid-type")
	}
}
