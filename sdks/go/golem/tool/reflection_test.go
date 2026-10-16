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
	"reflect"
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

func bashRun(t *testing.T) (reflection.Command, func(types.TypedSchemaValue) error, func() tool.BashArgs) {
	t.Helper()
	meta, invoke, seen := tool.BashFixture(t)
	cmd, ok := reflection.ToolOf(meta).Command([]string{"run"})
	if !ok {
		t.Fatal("reflection does not find run")
	}
	return cmd, invoke, seen
}

// startBash starts the bash fixture's run command through a reflected client
// and returns the input it sent.
func startBash(t *testing.T, args map[string]any) (types.TypedSchemaValue, error) {
	t.Helper()
	meta, _, _ := tool.BashFixture(t)
	inputs := tool.RecordToolInputs(t)
	if _, err := reflection.ToolOf(meta).Bind().Start([]string{"run"}, args, nil); err != nil {
		return types.TypedSchemaValue{}, err
	}
	if len(*inputs) != 1 {
		t.Fatalf("sent %d inputs, want 1", len(*inputs))
	}
	return (*inputs)[0], nil
}

// TestReflectedToolFillsDeclaredDefaults — an argument that declares a default
// may be left out of the JSON, and the handler then sees that default; an
// optional option without one is none.
func TestReflectedToolFillsDeclaredDefaults(t *testing.T) {
	cmd, invoke, seen := bashRun(t)
	packed, err := cmd.Input().PackJSON(map[string]any{"script": "echo ok"})
	if err != nil {
		t.Fatalf("PackJSON: %v", err)
	}
	want := core.RecordValue{Fields: []core.SchemaValue{
		core.StringValue{Value: "echo ok"}, core.StringValue{Value: ""}, core.U32Value{Value: 600},
		core.OptionValue{}, core.BoolValue{Value: true},
	}}
	if !reflect.DeepEqual(packed, want) {
		t.Errorf("packed %#v, want %#v", packed, want)
	}

	input, err := startBash(t, map[string]any{"script": "echo ok"})
	if err != nil {
		t.Fatalf("Start: %v", err)
	}
	if err := invoke(input); err != nil {
		t.Fatal(err)
	}
	if s := seen(); s.Script != "echo ok" || s.Cwd != "" || s.Timeout != 600 || s.Shell.IsSome() || !s.Login {
		t.Errorf("the handler saw %+v", s)
	}
}

// TestReflectedToolKeepsExplicitArguments — a given argument wins over its
// default, taken as it is: a timeout is not converted or clamped, and an
// explicit null for an option is none.
func TestReflectedToolKeepsExplicitArguments(t *testing.T) {
	_, invoke, seen := bashRun(t)
	for _, tc := range []struct {
		args map[string]any
		want tool.BashArgs
	}{
		{map[string]any{"script": "ls", "timeout": 30}, tool.BashArgs{Script: "ls", Timeout: 30, Login: true}},
		{map[string]any{"script": "ls", "timeout": 120000, "cwd": "/tmp", "login": false, "shell": nil},
			tool.BashArgs{Script: "ls", Cwd: "/tmp", Timeout: 120000}},
		{map[string]any{"script": "ls", "shell": "zsh"},
			tool.BashArgs{Script: "ls", Timeout: 600, Shell: golem.Some("zsh"), Login: true}},
	} {
		input, err := startBash(t, tc.args)
		if err != nil {
			t.Fatalf("Start(%v): %v", tc.args, err)
		}
		if err := invoke(input); err != nil {
			t.Fatal(err)
		}
		if s := seen(); !reflect.DeepEqual(s, tc.want) {
			t.Errorf("%v: the handler saw %+v, want %+v", tc.args, s, tc.want)
		}
	}
}

// TestReflectedToolReportsAMissingRequiredArgument — a required argument has
// no default to fall back to, so leaving it out fails before sending, naming
// it; an unknown argument is still refused.
func TestReflectedToolReportsAMissingRequiredArgument(t *testing.T) {
	cmd, _, _ := bashRun(t)
	_, err := cmd.Input().PackJSON(map[string]any{"timeout": 5})
	var ve *core.ValidationError
	if !errors.As(err, &ve) || len(ve.Issues) != 1 || ve.Issues[0].Path != "script" ||
		ve.Issues[0].Message != `missing field "script"` {
		t.Errorf("got %v, want one missing-field issue at script", err)
	}

	meta, _, _ := tool.BashFixture(t)
	inputs := tool.RecordToolInputs(t)
	client := reflection.ToolOf(meta).Bind()
	if _, err := client.Start([]string{"run"}, map[string]any{}, nil); err == nil ||
		!strings.Contains(err.Error(), `missing field "script"`) || !errors.As(err, &ve) {
		t.Errorf("Start without script: %v", err)
	}
	if _, err := client.Start([]string{"run"}, map[string]any{"script": "ls", "timout": 5}, nil); err == nil ||
		!strings.Contains(err.Error(), `unknown field "timout"`) {
		t.Errorf("Start with an unknown argument: %v", err)
	}
	if len(*inputs) != 0 {
		t.Error("a call was sent despite failing validation")
	}
}

// TestReflectedToolInputSchemaStatesDefaults — the JSON Schema requires only
// the arguments without a default and states each default.
func TestReflectedToolInputSchemaStatesDefaults(t *testing.T) {
	cmd, _, _ := bashRun(t)
	data, err := cmd.Input().ToJSONSchemaBytes(false)
	if err != nil {
		t.Fatalf("ToJSONSchemaBytes: %v", err)
	}
	var doc map[string]any
	_ = json.Unmarshal(data, &doc)
	if got, _ := json.Marshal(doc["required"]); string(got) != `["script"]` {
		t.Errorf("required is %s, want only script", got)
	}
	props := doc["properties"].(map[string]any)
	for name, want := range map[string]any{"cwd": "", "timeout": float64(600), "login": true} {
		prop := props[name].(map[string]any)
		if got, has := prop["default"]; !has || got != want {
			t.Errorf("%s default is %v (present %v), want %v", name, got, has, want)
		}
	}
	for _, name := range []string{"script", "shell"} {
		if _, has := props[name].(map[string]any)["default"]; has {
			t.Errorf("%s gained a default", name)
		}
	}
}

// TestReflectedToolWireRecordKeepsItsTypes — filling defaults is a matter of
// the JSON surface only: the record sent is typed as before, and leaving the
// defaults out sends exactly what passing them explicitly sends.
func TestReflectedToolWireRecordKeepsItsTypes(t *testing.T) {
	cmd, _, _ := bashRun(t)
	if got := recordFieldNames(cmd.Input()); strings.Join(got, ",") != "script,cwd,timeout,shell,login" {
		t.Errorf("fields are %v", got)
	}
	omitted, err := startBash(t, map[string]any{"script": "echo ok"})
	if err != nil {
		t.Fatal(err)
	}
	explicit, err := startBash(t, map[string]any{"script": "echo ok", "cwd": "", "timeout": 600, "shell": nil, "login": true})
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(omitted, explicit) {
		t.Error("leaving the defaults out sent a different input from passing them")
	}
	root := omitted.Graph.TypeNodes[omitted.Graph.Root].Body
	if root.Tag() != types.SchemaTypeBodyRecordType {
		t.Fatal("the input graph root is not a record")
	}
	tags := map[string]uint8{}
	for _, f := range root.RecordType() {
		tags[f.Name] = omitted.Graph.TypeNodes[f.Body].Body.Tag()
	}
	want := map[string]uint8{
		"script": types.SchemaTypeBodyStringType, "cwd": types.SchemaTypeBodyStringType,
		"timeout": types.SchemaTypeBodyU32Type, "shell": types.SchemaTypeBodyOptionType,
		"login": types.SchemaTypeBodyBoolType,
	}
	if !reflect.DeepEqual(tags, want) {
		t.Errorf("wire field types %v, want %v", tags, want)
	}
	if err := cmd.Input().Validate(core.RecordValue{Fields: []core.SchemaValue{core.StringValue{Value: "x"}}}); err == nil {
		t.Error("Validate accepted a record that leaves out defaulted fields")
	}
}

// TestReflectedToolDefaultsGlobalsAndFlags — inherited globals, a defaulted
// positional and count and bool flags fall back to their defaults, while a
// tail and repeatable options still have to be given.
func TestReflectedToolDefaultsGlobalsAndFlags(t *testing.T) {
	meta, invoke, seen := tool.VcsFixture(t)
	inputs := tool.RecordToolInputs(t)
	client := reflection.ToolOf(meta).Bind()
	if _, err := client.Start([]string{"commit"}, map[string]any{
		"paths": []any{"z"}, "message": "m", "include": []any{}, "tags": []any{},
	}, nil); err != nil {
		t.Fatal(err)
	}
	if err := invoke((*inputs)[0]); err != nil {
		t.Fatal(err)
	}
	if s := seen(); s.Dir != "." || s.Verbose != 0 || s.Branch != "main" || s.Amend || !s.Signoff || s.Author.IsSome() {
		t.Errorf("defaults decoded as %+v", s)
	}

	_, err := client.Start([]string{"commit"}, map[string]any{"message": "m"}, nil)
	var ve *core.ValidationError
	if !errors.As(err, &ve) {
		t.Fatalf("got %v, want a validation error", err)
	}
	var missing []string
	for _, i := range ve.Issues {
		missing = append(missing, i.Path)
	}
	if !slices.Equal(missing, []string{"paths", "include", "tags"}) {
		t.Errorf("missing %v, want the tail and the repeatable options", missing)
	}
}

// A value sent once moved its cards to the host: sending it again fails
// rather than name handles the host already owns.
func TestDynamicToolClientSendsAValueOnce(t *testing.T) {
	card, err := golem.DecodeTypedValue[golem.PermissionCard](link.TypedValue(types.TypedSchemaValue{
		Graph: witOf(golem.EncodeTypedValue(golem.Unit{})).Graph,
		Value: types.SchemaValueTree{ValueNodes: []types.SchemaValueNode{types.MakeSchemaValueNodePermissionCardHandle(&types.PermissionCard{})}},
	}).(golem.TypedValue))
	if err != nil {
		t.Fatal(err)
	}
	input := golem.EncodeTypedValue(struct{ Card golem.PermissionCard }{card})
	calls := tool.RecordToolCalls(t, func([]string) (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
		return witTypes.None[types.TypedSchemaValue](), nil
	})
	client := reflection.BindTool("keys")
	if _, err := client.Call([]string{"get"}, input); err != nil {
		t.Fatalf("the first send failed: %v", err)
	}
	if _, err := client.Call([]string{"get"}, input); !errors.Is(err, golem.ErrPermissionCardTaken) || len(*calls) != 1 {
		t.Errorf("the second send gave %v after %d calls", err, len(*calls))
	}
}
