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
	"strconv"
	"strings"
	"testing"

	toolExports "github.com/golemcloud/golem/sdks/go/golem/internal/exports/export_golem_tool_guest"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	"github.com/golemcloud/golem/sdks/go/golem/schema"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

type GreetArgs struct {
	Name  Positional[string]
	Loud  Flag
	Times Opt[int32]
}

var greetProto = GreetArgs{
	Name:  Positional[string]{Doc: "who to greet", ValueName: "WHO"},
	Loud:  Flag{Short: 'l', Doc: "shout the greeting"},
	Times: Opt[int32]{Short: 'n', Doc: "how many times", Default: Some(int32(1))},
}

// invokeNoStreams invokes a command the way a host would for a body that
// declared neither stdin nor stdout.
func invokeNoStreams(d *definitions, e *toolEntry, path []string, input types.TypedSchemaValue) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
	return d.invokeCommand(e, path, input,
		newToolStdin(toolExports.Stdin{}), newToolStdout(toolExports.Stdout{}))
}

// buildToolFor registers a tool on an isolated definition set and derives its
// metadata, so each test sees only its own declarations.
func buildToolFor(t *testing.T, declare func(r *toolRegistry, d *definitions)) (toolCommon.Tool, *toolRegistry, *definitions) {
	t.Helper()
	r, d := newToolRegistry(), newDefinitions()
	declare(r, d)
	tools, ok := r.discover(d)
	if !ok {
		t.Fatalf("tool discovery failed: %s", allDefErrors(d.errs))
	}
	if len(tools) != 1 {
		t.Fatalf("discovered %d tools, want 1", len(tools))
	}
	return tools[0], r, d
}

// declareGreeter registers the greeter tool with a root body, used by most of
// the tests below.
func declareGreeter(r *toolRegistry, d *definitions) CommandDef[GreetArgs, string] {
	def := defineToolInto(r, d, "greeter", ToolSpec{
		Version: "1.0.0",
		Summary: "Greets people",
	})
	cmd := declareCommand[GreetArgs, string](r, d, def, nil, "", greetProto, []CommandOpt{
		Summary("Greet someone"),
	})
	handleCommandInto(r, d, cmd, func(ctx *ToolContext, in GreetArgs) string {
		greeting := strings.TrimSpace(strings.Repeat("hi "+in.Name.Get()+" ", int(in.Times.Get())))
		if in.Loud.Get() {
			greeting = strings.ToUpper(greeting)
		}
		return greeting
	})
	return cmd
}

// TestToolMetadataDescribesTheCommandTree — the metadata is the whole contract:
// a host that never sees the Go types drives the tool from this alone.
func TestToolMetadataDescribesTheCommandTree(t *testing.T) {
	tool, _, _ := buildToolFor(t, func(r *toolRegistry, d *definitions) { declareGreeter(r, d) })

	if tool.Version != "1.0.0" {
		t.Errorf("version %q, want 1.0.0", tool.Version)
	}
	if len(tool.Commands.Nodes) != 1 {
		t.Fatalf("command tree has %d nodes, want 1", len(tool.Commands.Nodes))
	}
	root := tool.Commands.Nodes[0]
	// The tool's identity is its root command name.
	if root.Name != "greeter" {
		t.Errorf("root command %q, want greeter", root.Name)
	}
	if root.Body.IsNone() {
		t.Fatal("root command has no body")
	}
	body := root.Body.Some()

	if len(body.Positionals.Fixed) != 1 {
		t.Fatalf("%d positionals, want 1", len(body.Positionals.Fixed))
	}
	pos := body.Positionals.Fixed[0]
	if pos.Name != "name" || pos.Doc.Summary != "who to greet" {
		t.Errorf("positional is %+v, want name/who to greet", pos)
	}
	// A positional is required unless declared Optional.
	if !pos.Required {
		t.Error("positional is not required by default")
	}
	if !pos.ValueName.IsSome() || pos.ValueName.Some() != "WHO" {
		t.Errorf("value name %v, want WHO", pos.ValueName)
	}

	if len(body.Options) != 1 || body.Options[0].Long != "times" {
		t.Fatalf("options are %+v, want one named times", body.Options)
	}
	opt := body.Options[0]
	if !opt.Short.IsSome() || opt.Short.Some() != 'n' {
		t.Errorf("option short form %v, want n", opt.Short)
	}
	if opt.Default.IsNone() {
		t.Error("declared default did not reach the metadata")
	}

	if len(body.Flags) != 1 || body.Flags[0].Long != "loud" {
		t.Fatalf("flags are %+v, want one named loud", body.Flags)
	}
	if !body.Flags[0].Short.IsSome() || body.Flags[0].Short.Some() != 'l' {
		t.Errorf("flag short form %v, want l", body.Flags[0].Short)
	}

	if body.Result.IsNone() {
		t.Fatal("command declares no result")
	}
	if tag := tool.Schema.TypeNodes[body.Result.Some().Type].Body.Tag(); tag != types.SchemaTypeBodyStringType {
		t.Errorf("result type tag %d, want string", tag)
	}
}

// TestToolArgumentTypesResolveInTheToolSchema — every type index a command body
// carries must point into the tool's own schema pool, which is what makes the
// metadata self-contained.
func TestToolArgumentTypesResolveInTheToolSchema(t *testing.T) {
	tool, _, _ := buildToolFor(t, func(r *toolRegistry, d *definitions) { declareGreeter(r, d) })
	body := tool.Commands.Nodes[0].Body.Some()
	pool := len(tool.Schema.TypeNodes)

	for _, idx := range []int32{
		body.Positionals.Fixed[0].Type,
		body.Options[0].Shape.Scalar(),
		body.Result.Some().Type,
	} {
		if idx < 0 || int(idx) >= pool {
			t.Errorf("type index %d is outside the tool's %d-node schema", idx, pool)
		}
	}
	if tag := tool.Schema.TypeNodes[body.Positionals.Fixed[0].Type].Body.Tag(); tag != types.SchemaTypeBodyStringType {
		t.Errorf("positional type tag %d, want string", tag)
	}
	if tag := tool.Schema.TypeNodes[body.Options[0].Shape.Scalar()].Body.Tag(); tag != types.SchemaTypeBodyS32Type {
		t.Errorf("option type tag %d, want s32", tag)
	}
}

// encodeToolArgs builds an invocation input the way a host would: a record with
// one value per declared argument, in declaration order.
func encodeToolArgs(t *testing.T, d *definitions, e *toolEntry, path []string, values ...any) types.TypedSchemaValue {
	t.Helper()
	ce := e.byPath[pathKey(path)]
	fields, ok := d.argFields(e.def.name, ce)
	if !ok {
		t.Fatalf("argument fields: %s", allDefErrors(d.errs))
	}
	if len(values) != len(fields) {
		t.Fatalf("%d values for %d arguments", len(values), len(fields))
	}
	var b valBuilder
	idxs := make([]int32, 0, len(fields))
	for i, f := range fields {
		idxs = append(idxs, f.codec.encode(&b, reflect.ValueOf(values[i])))
	}
	root := b.push(types.MakeSchemaValueNodeRecordValue(idxs))
	return types.TypedSchemaValue{
		Value: types.SchemaValueTree{ValueNodes: b.nodes, Root: root},
	}
}

func TestToolInvocationDecodesArgumentsAndEncodesTheResult(t *testing.T) {
	_, r, d := buildToolFor(t, func(r *toolRegistry, d *definitions) { declareGreeter(r, d) })
	e, _ := r.get("greeter")

	input := encodeToolArgs(t, d, e, nil, "ada", true, int32(2))
	got := invokeNoStreams(d, e, nil, input)
	if got.Tag() != witTypes.ResultOk {
		t.Fatalf("invoke failed: %+v", got.Err())
	}
	res := got.Ok()
	if res.Result.IsNone() {
		t.Fatal("invocation produced no result")
	}

	typed := res.Result.Some()
	out, err := schema.NewRef(typed.Graph).UnpackJSON(typed.Value)
	if err != nil {
		t.Fatalf("result is not readable: %v", err)
	}
	if out != "HI ADA HI ADA" {
		t.Errorf("result %v, want %q", out, "HI ADA HI ADA")
	}
}

// TestToolInvocationRejectsAnUnknownCommand — the path is caller-supplied, so a
// wrong one must be reported rather than dispatched somewhere plausible.
func TestToolInvocationRejectsAnUnknownCommand(t *testing.T) {
	_, r, d := buildToolFor(t, func(r *toolRegistry, d *definitions) { declareGreeter(r, d) })
	e, _ := r.get("greeter")

	got := invokeNoStreams(d, e, []string{"absent"}, types.TypedSchemaValue{})
	if got.Tag() != witTypes.ResultErr {
		t.Fatal("invoking an unknown command succeeded")
	}
	if tag := got.Err().Tag(); tag != types.ToolErrorInvalidCommandPath {
		t.Errorf("error tag %d, want invalid-command-path", tag)
	}
}

func TestToolInvocationRejectsMalformedInput(t *testing.T) {
	_, r, d := buildToolFor(t, func(r *toolRegistry, d *definitions) { declareGreeter(r, d) })
	e, _ := r.get("greeter")

	// A bare string where the body expects a record of three arguments.
	var b valBuilder
	root := b.push(types.MakeSchemaValueNodeStringValue("ada"))
	got := invokeNoStreams(d, e, nil, types.TypedSchemaValue{
		Value: types.SchemaValueTree{ValueNodes: b.nodes, Root: root},
	})
	if got.Tag() != witTypes.ResultErr {
		t.Fatal("invoking with a malformed input succeeded")
	}
	if tag := got.Err().Tag(); tag != types.ToolErrorInvalidInput {
		t.Errorf("error tag %d, want invalid-input", tag)
	}
}

// TestSubcommandsAreReachable — a tool may dispatch to named subcommands as
// well as run its own body.
func TestSubcommandsAreReachable(t *testing.T) {
	type EchoArgs struct {
		Text Positional[string]
	}
	tool, r, d := buildToolFor(t, func(r *toolRegistry, d *definitions) {
		def := defineToolInto(r, d, "multi", ToolSpec{Version: "0.1.0"})
		cmd := declareCommand[EchoArgs, string](r, d, def, []string{"echo"}, "echo", EchoArgs{}, []CommandOpt{
			Summary("Echo the argument"), Aliases("say"),
		})
		handleCommandInto(r, d, cmd, func(ctx *ToolContext, in EchoArgs) string {
			return strings.Join(append(ctx.CommandPath(), in.Text.Get()), ":")
		})
	})

	if len(tool.Commands.Nodes) != 2 {
		t.Fatalf("command tree has %d nodes, want 2", len(tool.Commands.Nodes))
	}
	if got := tool.Commands.Nodes[0].Subcommands; len(got) != 1 || got[0] != 1 {
		t.Errorf("root subcommands %v, want [1]", got)
	}
	sub := tool.Commands.Nodes[1]
	if sub.Name != "echo" || len(sub.Aliases) != 1 || sub.Aliases[0] != "say" {
		t.Errorf("subcommand is %+v, want echo/say", sub)
	}
	// The root has no body of its own here, only a subcommand.
	if tool.Commands.Nodes[0].Body.IsSome() {
		t.Error("root gained a body it never declared")
	}

	e, _ := r.get("multi")
	input := encodeToolArgs(t, d, e, []string{"echo"}, "hello")
	got := invokeNoStreams(d, e, []string{"echo"}, input)
	if got.Tag() != witTypes.ResultOk {
		t.Fatalf("invoking the subcommand failed: %+v", got.Err())
	}
	typed := got.Ok().Result.Some()
	out, err := schema.NewRef(typed.Graph).UnpackJSON(typed.Value)
	if err != nil {
		t.Fatalf("result is not readable: %v", err)
	}
	if out != "echo:hello" {
		t.Errorf("result %v, want echo:hello", out)
	}
}

func TestToolDeclarationErrors(t *testing.T) {
	t.Run("non-marker field", func(t *testing.T) {
		type Bad struct{ Name string }
		r, d := newToolRegistry(), newDefinitions()
		def := defineToolInto(r, d, "bad", ToolSpec{})
		cmd := declareCommand[Bad, string](r, d, def, nil, "", Bad{}, nil)
		handleCommandInto(r, d, cmd, func(*ToolContext, Bad) string { return "" })
		r.discover(d)
		mustDefErr(t, d, "must be golem.Positional, golem.Opt or golem.Flag")
	})

	t.Run("missing handler", func(t *testing.T) {
		r, d := newToolRegistry(), newDefinitions()
		def := defineToolInto(r, d, "bare", ToolSpec{})
		declareCommand[GreetArgs, string](r, d, def, nil, "", greetProto, nil)
		r.discover(d)
		mustDefErr(t, d, "has no handler")
	})

	t.Run("duplicate tool", func(t *testing.T) {
		r, d := newToolRegistry(), newDefinitions()
		defineToolInto(r, d, "dup", ToolSpec{})
		defineToolInto(r, d, "dup", ToolSpec{})
		mustDefErr(t, d, "tool already defined")
	})

	t.Run("duplicate command", func(t *testing.T) {
		r, d := newToolRegistry(), newDefinitions()
		def := defineToolInto(r, d, "dup2", ToolSpec{})
		declareCommand[GreetArgs, string](r, d, def, nil, "", greetProto, nil)
		declareCommand[GreetArgs, string](r, d, def, nil, "", greetProto, nil)
		mustDefErr(t, d, "command already declared")
	})

	t.Run("two handlers", func(t *testing.T) {
		r, d := newToolRegistry(), newDefinitions()
		def := defineToolInto(r, d, "twice", ToolSpec{})
		cmd := declareCommand[GreetArgs, string](r, d, def, nil, "", greetProto, nil)
		handleCommandInto(r, d, cmd, func(*ToolContext, GreetArgs) string { return "" })
		handleCommandInto(r, d, cmd, func(*ToolContext, GreetArgs) string { return "" })
		mustDefErr(t, d, "already has a handler")
	})
}

// TestOptionShapes — the four wire shapes an option can take, each selected by
// its declaration rather than inferred from the payload type alone.
func TestOptionShapes(t *testing.T) {
	type ShapeArgs struct {
		// --name VALUE; the value is mandatory when the option appears.
		Plain Opt[string]
		// --signed, meaning the default, or --signed=mode.
		Signed Opt[string]
		// -e a -e b, collecting into a list.
		Include Opt[[]string]
		// -c a=1 -c b=2, collecting into a map.
		Config Opt[map[string]int32]
	}
	proto := ShapeArgs{
		Signed:  Opt[string]{ValueOptional: true, Default: Some("on")},
		Include: Opt[[]string]{Short: 'e', Repeatable: Repeated()},
		Config:  Opt[map[string]int32]{Short: 'c', Repeatable: Delimited(','), DuplicateKeys: LastKeyWins},
	}

	tool, _, _ := buildToolFor(t, func(r *toolRegistry, d *definitions) {
		def := defineToolInto(r, d, "shapes", ToolSpec{Version: "0.1.0"})
		cmd := declareCommand[ShapeArgs, Unit](r, d, def, nil, "", proto, nil)
		handleCommandInto(r, d, cmd, func(*ToolContext, ShapeArgs) Unit { return Unit{} })
	})

	body := tool.Commands.Nodes[0].Body.Some()
	byName := map[string]toolCommon.OptionSpec{}
	for _, o := range body.Options {
		byName[o.Long] = o
	}

	if got := byName["plain"].Shape.Tag(); got != toolCommon.OptionShapeScalar {
		t.Errorf("plain shape tag %d, want scalar", got)
	}
	if got := byName["signed"].Shape.Tag(); got != toolCommon.OptionShapeOptionalScalar {
		t.Errorf("signed shape tag %d, want optional-scalar", got)
	}

	include := byName["include"]
	if got := include.Shape.Tag(); got != toolCommon.OptionShapeRepeatableList {
		t.Fatalf("include shape tag %d, want repeatable-list", got)
	}
	list := include.Shape.RepeatableList()
	if list.Repetition.Tag() != toolCommon.RepetitionRepeated {
		t.Errorf("include repetition tag %d, want repeated", list.Repetition.Tag())
	}
	// The collected value is a list, so item-type is the *element* type.
	if tag := tool.Schema.TypeNodes[list.ItemType].Body.Tag(); tag != types.SchemaTypeBodyStringType {
		t.Errorf("include item type tag %d, want string", tag)
	}

	config := byName["config"]
	if got := config.Shape.Tag(); got != toolCommon.OptionShapeRepeatableMap {
		t.Fatalf("config shape tag %d, want repeatable-map", got)
	}
	m := config.Shape.RepeatableMap()
	if m.Repetition.Tag() != toolCommon.RepetitionDelimited || m.Repetition.Delimited() != ',' {
		t.Errorf("config repetition is %+v, want delimited by ','", m.Repetition)
	}
	if m.DuplicateKeyPolicy != toolCommon.DuplicateKeyPolicyLastWins {
		t.Errorf("config duplicate-key policy %d, want last-wins", m.DuplicateKeyPolicy)
	}
	// map-type points at the map node itself, never a list of tuples.
	if tag := tool.Schema.TypeNodes[m.MapType].Body.Tag(); tag != types.SchemaTypeBodyMapType {
		t.Errorf("config map type tag %d, want map", tag)
	}
}

// TestRepeatableOptionsRoundTrip — a repeatable option's collected value is
// just its declared type, so decoding needs no special case.
func TestRepeatableOptionsRoundTrip(t *testing.T) {
	type CollectArgs struct {
		Include Opt[[]string]
		Config  Opt[map[string]int32]
	}
	proto := CollectArgs{
		Include: Opt[[]string]{Short: 'e', Repeatable: Repeated()},
		Config:  Opt[map[string]int32]{Short: 'c', Repeatable: Repeated()},
	}
	_, r, d := buildToolFor(t, func(r *toolRegistry, d *definitions) {
		def := defineToolInto(r, d, "collect", ToolSpec{Version: "0.1.0"})
		cmd := declareCommand[CollectArgs, string](r, d, def, nil, "", proto, nil)
		handleCommandInto(r, d, cmd, func(_ *ToolContext, in CollectArgs) string {
			return strings.Join(in.Include.Get(), "+") + "/" + strconv.Itoa(int(in.Config.Get()["n"]))
		})
	})
	e, _ := r.get("collect")

	input := encodeToolArgs(t, d, e, nil, []string{"a", "b"}, map[string]int32{"n": 7})
	got := invokeNoStreams(d, e, nil, input)
	if got.Tag() != witTypes.ResultOk {
		t.Fatalf("invoke failed: %+v", got.Err())
	}
	typed := got.Ok().Result.Some()
	out, err := schema.NewRef(typed.Graph).UnpackJSON(typed.Value)
	if err != nil {
		t.Fatalf("result is not readable: %v", err)
	}
	if out != "a+b/7" {
		t.Errorf("result %v, want a+b/7", out)
	}
}

func TestOptionShapeDeclarationErrors(t *testing.T) {
	t.Run("value-optional without a default", func(t *testing.T) {
		type Args struct{ Signed Opt[string] }
		r, d := newToolRegistry(), newDefinitions()
		def := defineToolInto(r, d, "bare", ToolSpec{})
		cmd := declareCommand[Args, Unit](r, d, def, nil, "", Args{
			Signed: Opt[string]{ValueOptional: true},
		}, nil)
		handleCommandInto(r, d, cmd, func(*ToolContext, Args) Unit { return Unit{} })
		r.discover(d)
		mustDefErr(t, d, "ValueOptional but declares no Default")
	})

	t.Run("repeatable into a scalar", func(t *testing.T) {
		type Args struct{ Include Opt[string] }
		r, d := newToolRegistry(), newDefinitions()
		def := defineToolInto(r, d, "badrep", ToolSpec{})
		cmd := declareCommand[Args, Unit](r, d, def, nil, "", Args{
			Include: Opt[string]{Repeatable: Repeated()},
		}, nil)
		handleCommandInto(r, d, cmd, func(*ToolContext, Args) Unit { return Unit{} })
		r.discover(d)
		mustDefErr(t, d, "use a slice or a map")
	})

	t.Run("both shapes at once", func(t *testing.T) {
		type Args struct{ Include Opt[[]string] }
		r, d := newToolRegistry(), newDefinitions()
		def := defineToolInto(r, d, "both", ToolSpec{})
		cmd := declareCommand[Args, Unit](r, d, def, nil, "", Args{
			Include: Opt[[]string]{ValueOptional: true, Repeatable: Repeated(), Default: Some([]string{"a"})},
		}, nil)
		handleCommandInto(r, d, cmd, func(*ToolContext, Args) Unit { return Unit{} })
		r.discover(d)
		mustDefErr(t, d, "an option has one shape")
	})
}

// TestDefinitionErrorsTravelAsInvalidResult — a broken declaration is reported
// through the same variant the TypeScript SDK uses, and never as custom-error,
// whose name field is reserved for a tool's own declared error cases.
func TestDefinitionErrorsTravelAsInvalidResult(t *testing.T) {
	type Bad struct{ Name string }
	r, d := newToolRegistry(), newDefinitions()
	def := defineToolInto(r, d, "bad", ToolSpec{})
	cmd := declareCommand[Bad, string](r, d, def, nil, "", Bad{}, nil)
	handleCommandInto(r, d, cmd, func(*ToolContext, Bad) string { return "" })
	if _, ok := r.discover(d); ok {
		t.Fatal("discovery accepted a broken declaration")
	}

	err := toolDefinitionError(d)
	if err.Tag() != types.ToolErrorInvalidResult {
		t.Fatalf("error tag %d, want invalid-result", err.Tag())
	}
	if !strings.Contains(err.InvalidResult(), "must be golem.Positional") {
		t.Errorf("message does not name the problem: %q", err.InvalidResult())
	}
}
