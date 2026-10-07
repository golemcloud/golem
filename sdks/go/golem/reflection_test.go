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
	"strings"
	"testing"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

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
