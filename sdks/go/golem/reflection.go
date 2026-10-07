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
	"fmt"
	"io"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	"github.com/golemcloud/golem/sdks/go/golem/internal/witschema"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// packJSONTree validates canonical JSON against a type and renders it for the
// wire.
func packJSONTree(ref core.Ref, value any) (types.SchemaValueTree, error) {
	built, err := ref.PackJSON(value)
	if err != nil {
		return types.SchemaValueTree{}, err
	}
	return witschema.ValueToWit(built)
}

// ReflectedTool is an immutable snapshot of a tool deployed in the caller's
// environment.
type ReflectedTool struct {
	lookupName string
	wit        toolCommon.Tool
	conv       witschema.Converted
	convErr    error
}

// newReflectedTool converts a discovered tool's schema up front, on the same
// terms as an agent type.
func newReflectedTool(lookupName string, wit toolCommon.Tool) ReflectedTool {
	conv, err := witschema.GraphToCore(wit.Schema)
	return ReflectedTool{lookupName: lookupName, wit: wit, conv: conv, convErr: err}
}

// Name returns the name a client binds to, which is stable across adapters.
func (r ReflectedTool) Name() string { return r.lookupName }

// Version returns the tool's own version.
func (r ReflectedTool) Version() string { return r.wit.Version }

// Schema returns the tool's type pool. Its root is a placeholder: command
// bodies index into it.
func (r ReflectedTool) Schema() (core.Ref, error) {
	if r.convErr != nil {
		return core.Ref{}, r.convErr
	}
	return core.NewRef(r.conv.Graph), nil
}

// Root returns the tool's root command.
func (r ReflectedTool) Root() ReflectedCommand {
	return ReflectedCommand{
		conv: r.conv, convErr: r.convErr, witGraph: r.wit.Schema,
		tree: r.wit.Commands, index: 0, chain: []int32{0},
	}
}

// Command resolves a command path from the root, following subcommand names and
// aliases. A path that names no command reports false.
func (r ReflectedTool) Command(path []string) (ReflectedCommand, bool) {
	at := r.Root()
	for _, segment := range path {
		next, found := at.Subcommand(segment)
		if !found {
			return ReflectedCommand{}, false
		}
		at = next
	}
	return at, true
}

// Commands returns every command in the tool, each with its path from the root,
// which is how a caller enumerates what it may invoke.
func (r ReflectedTool) Commands() []ReflectedCommand {
	var out []ReflectedCommand
	var walk func(c ReflectedCommand)
	walk = func(c ReflectedCommand) {
		out = append(out, c)
		for _, sub := range c.Subcommands() {
			walk(sub)
		}
	}
	walk(r.Root())
	return out
}

// ReflectedCommand is one node of a tool's command tree.
type ReflectedCommand struct {
	conv    witschema.Converted
	convErr error
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

func (c ReflectedCommand) node() toolCommon.CommandNode { return c.tree.Nodes[c.index] }

// Name returns the command's own name.
func (c ReflectedCommand) Name() string { return c.node().Name }

// Path returns the command's path from the tool's root; empty addresses the
// root's own body.
func (c ReflectedCommand) Path() []string { return append([]string(nil), c.path...) }

// Description returns the command's documentation.
func (c ReflectedCommand) Description() string { return c.node().Doc.Summary }

// Subcommands returns the command's children.
func (c ReflectedCommand) Subcommands() []ReflectedCommand {
	kids := c.node().Subcommands
	out := make([]ReflectedCommand, 0, len(kids))
	for _, idx := range kids {
		out = append(out, ReflectedCommand{
			conv: c.conv, convErr: c.convErr, witGraph: c.witGraph,
			tree: c.tree, index: idx,
			path:  append(append([]string(nil), c.path...), c.tree.Nodes[idx].Name),
			chain: append(append([]int32(nil), c.chain...), idx),
		})
	}
	return out
}

// Subcommand resolves one child by name or alias.
func (c ReflectedCommand) Subcommand(name string) (ReflectedCommand, bool) {
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
	return ReflectedCommand{}, false
}

// Callable reports whether the command has a body of its own. A node that only
// dispatches to subcommands stays discoverable but cannot be invoked.
func (c ReflectedCommand) Callable() bool { return c.node().Body.IsSome() }

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
func (c ReflectedCommand) canonicalFields() ([]canonicalField, error) {
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
// PackJSON, and render it with ToJSONSchema.
func (c ReflectedCommand) Input() (core.Ref, error) {
	if c.convErr != nil {
		return core.Ref{}, c.convErr
	}
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
func (c ReflectedCommand) inputGraph(fields []canonicalField) types.SchemaGraph {
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

// Output returns the command's result type, none when it produces none.
func (c ReflectedCommand) Output() (Option[core.Ref], error) {
	if c.convErr != nil {
		return None[core.Ref](), c.convErr
	}
	if !c.Callable() {
		return None[core.Ref](), fmt.Errorf("golem: command %q has no body", c.Name())
	}
	body := c.node().Body.Some()
	if body.Result.IsNone() {
		return None[core.Ref](), nil
	}
	ref, err := c.conv.Ref(body.Result.Some().Type)
	if err != nil {
		return None[core.Ref](), err
	}
	return Some(ref), nil
}

// ReflectedError is a failure a command declares.
type ReflectedError struct {
	Name        string
	Kind        ToolErrorKind
	ExitCode    uint8
	Summary     string
	Description string
	// Payload is the error's payload type, if it carries one.
	Payload Option[core.Ref]
}

// Errors returns the failures the command declares.
func (c ReflectedCommand) Errors() []ReflectedError {
	if !c.Callable() {
		return nil
	}
	var out []ReflectedError
	for _, e := range c.node().Body.Some().Errors {
		r := ReflectedError{
			Name:        e.Name,
			Kind:        UsageError,
			ExitCode:    e.ExitCode,
			Summary:     e.Doc.Summary,
			Description: e.Doc.Description,
			Payload:     None[core.Ref](),
		}
		if e.Kind == toolCommon.ErrorKindRuntimeError {
			r.Kind = RuntimeError
		}
		if e.Payload.IsSome() && c.convErr == nil {
			if ref, err := c.conv.Ref(e.Payload.Some()); err == nil {
				r.Payload = Some(ref)
			}
		}
		out = append(out, r)
	}
	return out
}

// pack validates named arguments against the command's input record and
// renders them with a graph rooted at that record, which is what the host
// checks an invocation against.
func (c ReflectedCommand) pack(args map[string]any) (types.TypedSchemaValue, error) {
	input, err := c.Input()
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

// ReflectedToolClient invokes a discovered tool. Arguments are packed and
// validated against the snapshot before anything is sent.
type ReflectedToolClient struct {
	tool ReflectedTool
}

// Tool returns the snapshot this client was built from.
func (c *ReflectedToolClient) Tool() ReflectedTool { return c.tool }

func (c *ReflectedToolClient) command(path []string) (ReflectedCommand, error) {
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
func (c *ReflectedToolClient) Call(path []string, args map[string]any) (any, error) {
	inv, err := c.Start(path, args, nil)
	if err != nil {
		return nil, err
	}
	return inv.Wait()
}

// Start starts a command with named arguments and the given standard input,
// which may be nil, and returns the running invocation with every output the
// command declares.
func (c *ReflectedToolClient) Start(path []string, args map[string]any, stdin io.Reader) (*ToolInvocation[any], error) {
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
		return nil, &ToolCallError{Tool: c.tool.Name(), CommandPath: path, Kind: ToolCallInvalidInput,
			Message: "the command requires standard input"}
	}
	output, err := cmd.Output()
	if err != nil {
		return nil, err
	}
	name := c.tool.Name()
	call, err := startToolCall(name, path, input, stdin, ToolStreams{Stdout: body.Stdout.IsSome(), Stderr: body.Stderr.IsSome()})
	if err != nil {
		return nil, err
	}
	return newInvocation(call, func(call toolCall) (any, error) {
		res, rpcErr := call.wait()
		if rpcErr != nil {
			return nil, toolCallErrorFromWit(name, path, *rpcErr)
		}
		out, declared := output.Get()
		value, has := optionFromWit(res).Get()
		switch {
		case has && !declared:
			return nil, &ToolCallError{Tool: name, CommandPath: path, Kind: ToolCallInvalidResult,
				Message: "the command returned a value but declares none"}
		case !has && declared:
			return nil, &ToolCallError{Tool: name, CommandPath: path, Kind: ToolCallInvalidResult,
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
		return nil, &ToolCallError{Tool: name, CommandPath: path, Kind: ToolCallInvalidResult, Message: err.Error()}
	}), nil
}
