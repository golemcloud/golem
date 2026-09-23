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
	"strings"
	"testing"

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
	got := d.invokeCommand(e, nil, input)
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

	got := d.invokeCommand(e, []string{"absent"}, types.TypedSchemaValue{})
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
	got := d.invokeCommand(e, nil, types.TypedSchemaValue{
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
	got := d.invokeCommand(e, []string{"echo"}, input)
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
