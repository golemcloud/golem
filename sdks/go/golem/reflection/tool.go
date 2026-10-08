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

package reflection

import (
	"fmt"
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
	"github.com/golemcloud/golem/sdks/go/golem/tool"
	"io"
	"strings"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	"github.com/golemcloud/golem/sdks/go/golem/internal/witschema"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Tool is an immutable snapshot of a tool deployed in the caller's
// environment.
type Tool struct {
	lookupName string
	wit        toolCommon.Tool
	conv       witschema.Converted
}

// newTool converts a discovered tool's schema once, when the snapshot is
// taken, and checks every command's signature against it, on the same terms as
// an agent type: a tool the host describes is well-formed, so one that does not
// convert is a broken host and panics.
func newTool(lookupName string, wit toolCommon.Tool) Tool {
	conv, err := witschema.GraphToCore(wit.Schema)
	if err != nil {
		panic(fmt.Errorf("golem: tool %q has a malformed schema: %w", lookupName, err))
	}
	r := Tool{lookupName: lookupName, wit: wit, conv: conv}
	for _, c := range r.Commands() {
		if !c.Callable() {
			continue
		}
		if _, err := c.input(); err != nil {
			panic(fmt.Errorf("golem: tool %q command %s: %w", lookupName, commandLabel(c.path), err))
		}
		if _, err := c.output(); err != nil {
			panic(fmt.Errorf("golem: tool %q command %s result: %w", lookupName, commandLabel(c.path), err))
		}
	}
	return r
}

// Name returns the name a client binds to, which is stable across adapters.
func (r Tool) Name() string { return r.lookupName }

// Version returns the tool's own version.
func (r Tool) Version() string { return r.wit.Version }

// Schema returns the tool's type pool. Its root is a placeholder: command
// bodies index into it.
func (r Tool) Schema() core.Ref { return core.NewRef(r.conv.Graph) }

// Root returns the tool's root command.
func (r Tool) Root() Command {
	return Command{
		conv: r.conv, witGraph: r.wit.Schema,
		tree: r.wit.Commands, index: 0, chain: []int32{0},
	}
}

// Command resolves a command path from the root, following subcommand names and
// aliases. A path that names no command reports false.
func (r Tool) Command(path []string) (Command, bool) {
	at := r.Root()
	for _, segment := range path {
		next, found := at.Subcommand(segment)
		if !found {
			return Command{}, false
		}
		at = next
	}
	return at, true
}

// Commands returns every command in the tool, each with its path from the root,
// which is how a caller enumerates what it may invoke.
func (r Tool) Commands() []Command {
	var out []Command
	var walk func(c Command)
	walk = func(c Command) {
		out = append(out, c)
		for _, sub := range c.Subcommands() {
			walk(sub)
		}
	}
	walk(r.Root())
	return out
}

// Command is one node of a tool's command tree.
type Command struct {
	conv witschema.Converted
	// witGraph is the tool's schema as the wire carries it. An invocation
	// travels as a typed value, so the graph that goes with it has to be the
	// wire's own, not the converted one.
	witGraph types.SchemaGraph
	tree     toolCommon.CommandTree
	index    int32
	path     []string
	// chain is the node indices from the root to this command, whose globals
	// the command inherits.
	chain []int32
}

func (c Command) node() toolCommon.CommandNode { return c.tree.Nodes[c.index] }

// Name returns the command's own name.
func (c Command) Name() string { return c.node().Name }

// Path returns the command's path from the tool's root; empty addresses the
// root's own body.
func (c Command) Path() []string { return append([]string(nil), c.path...) }

// Description returns the command's documentation.
func (c Command) Description() string { return c.node().Doc.Summary }

// Subcommands returns the command's children.
func (c Command) Subcommands() []Command {
	kids := c.node().Subcommands
	out := make([]Command, 0, len(kids))
	for _, idx := range kids {
		out = append(out, Command{
			conv: c.conv, witGraph: c.witGraph,
			tree: c.tree, index: idx,
			path:  append(append([]string(nil), c.path...), c.tree.Nodes[idx].Name),
			chain: append(append([]int32(nil), c.chain...), idx),
		})
	}
	return out
}

// Subcommand resolves one child by name or alias.
func (c Command) Subcommand(name string) (Command, bool) {
	for _, sub := range c.Subcommands() {
		node := sub.node()
		if node.Name == name {
			return sub, true
		}
		for _, alias := range node.Aliases {
			if alias == name {
				return sub, true
			}
		}
	}
	return Command{}, false
}

// Callable reports whether the command has a body of its own. A node that only
// dispatches to subcommands stays discoverable but cannot be invoked.
func (c Command) Callable() bool { return c.node().Body.IsSome() }

// canonicalField is one field of a command's canonical input record: the
// type node the metadata names, and how the record wraps it.
type canonicalField struct {
	name string
	node int32
	wrap canonicalWrap
}

type canonicalWrap uint8

const (
	wrapNone canonicalWrap = iota
	wrapOption
	wrapList
	wrapBool
	wrapCount
)

// canonicalFields lists the command's canonical input record: inherited
// globals root first (options before flags per node), then the body's
// positionals, tail, options and flags. A field is wrapped in an option when it
// is neither required nor defaulted, and collects into a list for a tail or a
// repeatable option.
func (c Command) canonicalFields() ([]canonicalField, error) {
	if !c.Callable() {
		return nil, fmt.Errorf("golem: command %q has no body", c.Name())
	}
	isOption := func(idx int32) bool {
		return int(idx) < len(c.witGraph.TypeNodes) &&
			c.witGraph.TypeNodes[idx].Body.Tag() == types.SchemaTypeBodyOptionType
	}
	optional := func(required, defaulted bool, idx int32) canonicalWrap {
		if !required && !defaulted && !isOption(idx) {
			return wrapOption
		}
		return wrapNone
	}
	var out []canonicalField
	option := func(o toolCommon.OptionSpec) {
		switch o.Shape.Tag() {
		case toolCommon.OptionShapeRepeatableList:
			out = append(out, canonicalField{o.Long, o.Shape.RepeatableList().ItemType, wrapList})
		case toolCommon.OptionShapeRepeatableMap:
			out = append(out, canonicalField{o.Long, o.Shape.RepeatableMap().MapType, wrapNone})
		case toolCommon.OptionShapeOptionalScalar:
			n := o.Shape.OptionalScalar()
			out = append(out, canonicalField{o.Long, n, optional(o.Required, o.Default.IsSome(), n)})
		default:
			n := o.Shape.Scalar()
			out = append(out, canonicalField{o.Long, n, optional(o.Required, o.Default.IsSome(), n)})
		}
	}
	flag := func(f toolCommon.FlagSpec) {
		if f.Shape.Tag() == toolCommon.FlagShapeCountFlag {
			out = append(out, canonicalField{f.Long, -1, wrapCount})
		} else {
			out = append(out, canonicalField{f.Long, -1, wrapBool})
		}
	}
	for _, idx := range c.chain {
		g := c.tree.Nodes[idx].Globals
		for _, o := range g.Options {
			option(o)
		}
		for _, f := range g.Flags {
			flag(f)
		}
	}
	body := c.node().Body.Some()
	for _, p := range body.Positionals.Fixed {
		out = append(out, canonicalField{p.Name, p.Type, optional(p.Required, p.Default.IsSome(), p.Type)})
	}
	if body.Positionals.Tail.IsSome() {
		t := body.Positionals.Tail.Some()
		out = append(out, canonicalField{t.Name, t.ItemType, wrapList})
	}
	for _, o := range body.Options {
		option(o)
	}
	for _, f := range body.Flags {
		flag(f)
	}
	return out, nil
}

// Input returns the command's canonical input record: inherited globals,
// positionals, the tail, options and flags. Pack canonical JSON with its
// PackJSON, and render it with ToJSONSchema. Only a [Command.Callable] command
// has one; Input panics on a node that only dispatches to subcommands.
func (c Command) Input() core.Ref {
	ref, err := c.input()
	if err != nil {
		panic(err)
	}
	return ref
}

func (c Command) input() (core.Ref, error) {
	fields, err := c.canonicalFields()
	if err != nil {
		return core.Ref{}, err
	}
	record := make([]core.NamedField, 0, len(fields))
	for _, f := range fields {
		var t core.SchemaType
		switch f.wrap {
		case wrapBool:
			t = core.SchemaType{Body: core.BoolType{}}
		case wrapCount:
			t = core.SchemaType{Body: core.U32Type{}}
		default:
			t, err = c.conv.At(f.node)
			if err != nil {
				return core.Ref{}, fmt.Errorf("golem: argument %q: %w", f.name, err)
			}
			switch f.wrap {
			case wrapOption:
				t = core.SchemaType{Body: core.OptionType{Inner: t}}
			case wrapList:
				t = core.SchemaType{Body: core.ListType{Element: t}}
			}
		}
		record = append(record, core.NamedField{Name: f.name, Body: t})
	}
	return core.NewRefAt(c.conv.Graph, core.SchemaType{Body: core.RecordType{Fields: record}}), nil
}

// inputGraph is the tool's wire schema extended with the command's canonical
// input record as its root, which is what the host checks an invocation
// against.
func (c Command) inputGraph(fields []canonicalField) types.SchemaGraph {
	nodes := append([]types.SchemaTypeNode(nil), c.witGraph.TypeNodes...)
	add := func(b types.SchemaTypeBody) int32 {
		nodes = append(nodes, types.SchemaTypeNode{Body: b})
		return int32(len(nodes) - 1)
	}
	record := make([]types.NamedFieldType, 0, len(fields))
	for _, f := range fields {
		idx := f.node
		switch f.wrap {
		case wrapOption:
			idx = add(types.MakeSchemaTypeBodyOptionType(f.node))
		case wrapList:
			idx = add(types.MakeSchemaTypeBodyListType(f.node))
		case wrapBool:
			idx = add(types.MakeSchemaTypeBodyBoolType())
		case wrapCount:
			idx = add(types.MakeSchemaTypeBodyU32Type(witTypes.None[types.NumericRestrictions]()))
		}
		record = append(record, types.NamedFieldType{Name: f.name, Body: idx})
	}
	root := add(types.MakeSchemaTypeBodyRecordType(record))
	return types.SchemaGraph{TypeNodes: nodes, Defs: c.witGraph.Defs, Root: root}
}

// Output returns the command's result type, none when it produces none or has
// no body of its own.
func (c Command) Output() golem.Option[core.Ref] {
	ref, err := c.output()
	if err != nil {
		panic(err)
	}
	return ref
}

func (c Command) output() (golem.Option[core.Ref], error) {
	if !c.Callable() {
		return golem.None[core.Ref](), nil
	}
	body := c.node().Body.Some()
	if body.Result.IsNone() {
		return golem.None[core.Ref](), nil
	}
	ref, err := c.conv.Ref(body.Result.Some().Type)
	if err != nil {
		return golem.None[core.Ref](), err
	}
	return golem.Some(ref), nil
}

// ErrorCase is a failure a command declares.
type ErrorCase struct {
	Name        string
	Kind        tool.ErrorKind
	ExitCode    uint8
	Summary     string
	Description string
	// Payload is the error's payload type, if it carries one.
	Payload golem.Option[core.Ref]
}

// Errors returns the failures the command declares.
func (c Command) Errors() []ErrorCase {
	if !c.Callable() {
		return nil
	}
	var out []ErrorCase
	for _, e := range c.node().Body.Some().Errors {
		r := ErrorCase{
			Name:        e.Name,
			Kind:        tool.UsageError,
			ExitCode:    e.ExitCode,
			Summary:     e.Doc.Summary,
			Description: e.Doc.Description,
			Payload:     golem.None[core.Ref](),
		}
		if e.Kind == toolCommon.ErrorKindRuntimeError {
			r.Kind = tool.RuntimeError
		}
		if e.Payload.IsSome() {
			if ref, err := c.conv.Ref(e.Payload.Some()); err == nil {
				r.Payload = golem.Some(ref)
			}
		}
		out = append(out, r)
	}
	return out
}

// pack validates named arguments against the command's input record and
// renders them with a graph rooted at that record, which is what the host
// checks an invocation against.
func (c Command) pack(args map[string]any) (types.TypedSchemaValue, error) {
	input, err := c.input()
	if err != nil {
		return types.TypedSchemaValue{}, err
	}
	tree, err := packJSONTree(input, args)
	if err != nil {
		return types.TypedSchemaValue{}, err
	}
	fields, err := c.canonicalFields()
	if err != nil {
		return types.TypedSchemaValue{}, err
	}
	return types.TypedSchemaValue{Graph: c.inputGraph(fields), Value: tree}, nil
}

// ToolClient invokes a discovered tool. Arguments are packed and
// validated against the snapshot before anything is sent.
type ToolClient struct {
	tool Tool
}

// Tool returns the snapshot this client was built from.
func (c *ToolClient) Tool() Tool { return c.tool }

func (c *ToolClient) command(path []string) (Command, error) {
	cmd, found := c.tool.Command(path)
	if !found {
		return cmd, fmt.Errorf("golem: tool %q has no command %s", c.tool.Name(), commandLabel(path))
	}
	if !cmd.Callable() {
		// A namespace node stays discoverable so a caller can walk to its
		// children, but it has nothing to run.
		return cmd, fmt.Errorf("golem: tool %q command %s only dispatches to subcommands",
			c.tool.Name(), commandLabel(path))
	}
	return cmd, nil
}

// Call runs a command with named arguments and returns its result as canonical
// JSON, or nil when the command produces none; its outputs, if any, are
// discarded. Start a command to read them.
func (c *ToolClient) Call(path []string, args map[string]any) (any, error) {
	inv, err := c.Start(path, args, nil)
	if err != nil {
		return nil, err
	}
	return inv.Wait()
}

// Start starts a command with named arguments and the given standard input,
// which may be nil, and returns the running invocation with every output the
// command declares.
func (c *ToolClient) Start(path []string, args map[string]any, stdin io.Reader) (*tool.Invocation[any], error) {
	cmd, err := c.command(path)
	if err != nil {
		return nil, err
	}
	input, err := cmd.pack(args)
	if err != nil {
		return nil, fmt.Errorf("golem: %s %s: %w", c.tool.Name(), commandLabel(path), err)
	}
	body := cmd.node().Body.Some()
	if body.Stdin.IsSome() && body.Stdin.Some().Required && stdin == nil {
		return nil, &tool.CallError{Tool: c.tool.Name(), CommandPath: path, Kind: tool.CallInvalidInput,
			Message: "the command requires standard input"}
	}
	output := cmd.Output()
	name := c.tool.Name()
	inv, err := link.StartToolCall(name, path, input, stdin,
		tool.Streams{Stdout: body.Stdout.IsSome(), Stderr: body.Stderr.IsSome()}, false,
		func(res witTypes.Option[types.TypedSchemaValue]) (any, error) {
			out, declared := output.Get()
			value, has := optionFromWit(res).Get()
			switch {
			case has && !declared:
				return nil, &tool.CallError{Tool: name, CommandPath: path, Kind: tool.CallInvalidResult,
					Message: "the command returned a value but declares none"}
			case !has && declared:
				return nil, &tool.CallError{Tool: name, CommandPath: path, Kind: tool.CallInvalidResult,
					Message: "the command returned nothing but declares a result"}
			case !has:
				return nil, nil
			}
			v, err := witschema.ValueToCore(value.Value)
			if err == nil {
				var unpacked any
				unpacked, err = out.UnpackJSON(v)
				if err == nil {
					return unpacked, nil
				}
			}
			return nil, &tool.CallError{Tool: name, CommandPath: path, Kind: tool.CallInvalidResult, Message: err.Error()}
		})
	if err != nil {
		return nil, err
	}
	return inv.(*tool.Invocation[any]), nil
}

func commandLabel(path []string) string {
	if len(path) == 0 {
		return "<root>"
	}
	return strings.Join(path, " ")
}

// ToolOf reads the metadata a universal middleware receives about the tool it
// wraps as a discovered tool.
func ToolOf(m tool.Metadata) Tool {
	name, wit := link.ToolMetadata(m)
	return newTool(name, wit)
}
