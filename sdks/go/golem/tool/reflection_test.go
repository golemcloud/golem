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

package tool_test

import (
	"encoding/json"
	"errors"
	"slices"
	"strings"
	"testing"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	"github.com/golemcloud/golem/sdks/go/golem/reflection"
	"github.com/golemcloud/golem/sdks/go/golem/tool"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

func recordFieldNames(ref core.Ref) []string {
	record, _ := ref.Type().Body.(core.RecordType)
	out := make([]string, 0, len(record.Fields))
	for _, f := range record.Fields {
		out = append(out, f.Name)
	}
	return out
}

func witOf(v golem.TypedValue) types.TypedSchemaValue { return link.TypedValueWit(v) }

type addArgs struct {
	Path    string
	Force   bool
	Retries int32
}

// TestToolSnapshotWalksTheCommandTree — a caller with no Go types for the tool
// navigates it by name, including through namespace nodes and aliases.
func TestToolSnapshotWalksTheCommandTree(t *testing.T) {
	r := reflection.ToolOf(tool.FilesMetadata(t))
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
	r := reflection.ToolOf(tool.FilesMetadata(t))
	add, _ := r.Command([]string{"index", "add"})
	input := add.Input()
	if got := recordFieldNames(input); strings.Join(got, ",") != "path,retries,force" {
		t.Errorf("fields are %v, want positionals then options then flags", got)
	}
	record := input.Type().Body.(core.RecordType)
	if _, isBool := record.Fields[2].Body.Body.(core.BoolType); !isBool {
		t.Errorf("the flag is typed %T instead of a bool", record.Fields[2].Body.Body)
	}
	if add.Output().IsNone() {
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

func TestReflectedToolClientCalls(t *testing.T) {
	r := reflection.ToolOf(tool.FilesMetadata(t))
	result := golem.EncodeTypedValue("/tmp/a")
	calls := tool.RecordToolCalls(t, func([]string) (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
		return witTypes.Some(witOf(result)), nil
	})
	client := r.Bind()

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
	r := reflection.ToolOf(tool.FilesMetadata(t))
	payload := golem.EncodeTypedValue("missing.txt")
	tool.RecordToolCalls(t, func([]string) (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
		e := types.MakeToolRpcErrorRemoteToolError(types.MakeToolErrorCustomError(types.CustomToolError{
			Name: "not-found", Payload: witOf(payload),
		}))
		return witTypes.None[types.TypedSchemaValue](), &e
	})
	client := r.Bind()
	_, err := client.Call([]string{"index", "add"}, map[string]any{"path": "/x", "force": false, "retries": 1})
	var ce *tool.CallError
	if !errors.As(err, &ce) || ce.Kind != tool.CallDeclaredError || ce.ErrorName != "not-found" {
		t.Fatalf("got %v", err)
	}
	if v, err := ce.Payload().JSON(); err != nil || v != "missing.txt" {
		t.Errorf("payload %v (%v)", v, err)
	}
}

// TestReflectedToolClientRefusesANamespace — a dispatch-only node is
// discoverable but has nothing to run.
func TestReflectedToolClientRefusesANamespace(t *testing.T) {
	r := reflection.ToolOf(tool.FilesMetadata(t))
	client := r.Bind()
	_, err := client.Call([]string{"index"}, nil)
	if err == nil || !strings.Contains(err.Error(), "only dispatches to subcommands") {
		t.Errorf("error is %v", err)
	}
}

func TestReflectedToolClientValidatesBeforeSending(t *testing.T) {
	r := reflection.ToolOf(tool.FilesMetadata(t))
	calls := tool.RecordToolCalls(t, func([]string) (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
		return witTypes.None[types.TypedSchemaValue](), nil
	})
	client := r.Bind()
	_, err := client.Call([]string{"index", "add"}, map[string]any{"path": "/tmp/a", "force": "yes", "retries": 1})
	if err == nil {
		t.Fatal("an invalid flag reached the target")
	}
	if len(*calls) != 0 {
		t.Error("the call was sent despite failing validation")
	}
}

func TestDynamicToolClientCalls(t *testing.T) {
	r := reflection.ToolOf(tool.FilesMetadata(t))
	add, _ := r.Command([]string{"index", "add"})
	input := golem.EncodeTypedValue(addArgs{Path: "/tmp/a", Retries: 1})
	_ = add
	result := golem.EncodeTypedValue("/tmp/a")
	calls := tool.RecordToolCalls(t, func([]string) (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
		return witTypes.Some(witOf(result)), nil
	})
	client := reflection.BindTool("files")
	got, err := client.Call([]string{"index", "add"}, input)
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

// TestReflectionPacksTheCanonicalRecord — a discovered command's input is the
// record the typed client sends, and what reflection packs decodes into the
// handler's own arguments.
func TestReflectionPacksTheCanonicalRecord(t *testing.T) {
	meta, invoke, seen := tool.VcsFixture(t)
	want := []string{"dir", "verbose", "branch", "paths", "message", "author", "include", "tags", "amend", "signoff"}
	cmd, ok := reflection.ToolOf(meta).Command([]string{"commit"})
	if !ok {
		t.Fatal("reflection does not find commit")
	}
	if got := recordFieldNames(cmd.Input()); !slices.Equal(got, want) {
		t.Errorf("reflected fields %v, want %v", got, want)
	}

	inputs := tool.RecordToolInputs(t)
	if _, err := reflection.ToolOf(meta).Bind().Start([]string{"commit"}, map[string]any{
		"dir": "/src", "verbose": 1, "branch": "dev", "paths": []any{"z"}, "message": "via reflection",
		"author": "ann", "include": []any{}, "tags": []any{}, "amend": false, "signoff": false,
	}, nil); err != nil {
		t.Fatal(err)
	}
	packed := (*inputs)[0]
	if root := packed.Graph.TypeNodes[packed.Graph.Root].Body; root.Tag() != types.SchemaTypeBodyRecordType {
		t.Fatal("reflection's input graph root is not a record")
	}
	if err := invoke(packed); err != nil {
		t.Fatal(err)
	}
	if s := seen(); s.Dir != "/src" || s.Branch != "dev" || s.Author.Unwrap() != "ann" || s.Signoff || s.Verbose != 1 {
		t.Errorf("reflection-packed arguments decoded as %+v", s)
	}
}

// TestReflectedCommandErrorsAreDescribed — a caller reading a tool learns its
// declared failures in the SDK's own terms, payload type included.
func TestReflectedCommandErrorsAreDescribed(t *testing.T) {
	errs := reflection.ToolOf(tool.LookupMetadata(t)).Root().Errors()
	if len(errs) != 2 {
		t.Fatalf("errors = %+v, want not-found and offline", errs)
	}
	byName := map[string]reflection.ErrorCase{}
	for _, e := range errs {
		byName[e.Name] = e
	}
	notFound, offline := byName["not-found"], byName["offline"]
	if notFound.Kind != tool.UsageError || notFound.ExitCode != 2 || notFound.Summary != "no such name" {
		t.Errorf("not-found = %+v", notFound)
	}
	if ref, has := notFound.Payload.Get(); !has {
		t.Error("not-found lost its payload type")
	} else if _, err := ref.PackJSON(map[string]any{"name": "x"}); err != nil {
		t.Errorf("the payload type does not accept its own shape: %v", err)
	}
	if offline.Kind != tool.RuntimeError || offline.ExitCode != 69 || offline.Payload.IsSome() {
		t.Errorf("offline = %+v", offline)
	}

}
