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

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	"github.com/golemcloud/golem/sdks/go/golem/internal/witschema"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Reflection.
//
// A reflected client is built from what the deployment says, not from a shared
// Go definition: discovery returns an immutable snapshot of an agent type, and
// that snapshot validates and packs every call made through it.
//
//	agentType, found := golem.DiscoverAgentType("Greeter")
//	if !found { ... }
//	client, err := agentType.Bind(map[string]any{"name": "ada"})
//	result, err := client.InvokeAndAwait("greet", map[string]any{"greeting": "hi"})
//
// Arguments and results are canonical JSON — ordinary Go values read against
// the snapshot's schema — because the caller has none of the target's types.
// Snapshots never refresh themselves; discover again when a newer deployment
// matters.

// ReflectedAgentType is an immutable snapshot of a deployed agent type.
type ReflectedAgentType struct {
	wit common.AgentType
	// conv is the snapshot's schema in the shared model, converted once when
	// the snapshot is taken rather than on every call. It also carries the
	// index side table, because the wire selects sub-schemas by node index and
	// the shared model has no indices.
	conv    witschema.Converted
	convErr error
}

// newReflectedAgentType converts a discovered agent type's schema up front. A
// malformed graph is remembered rather than raised here, so discovery stays a
// lookup and the failure surfaces where the schema is actually used.
func newReflectedAgentType(wit common.AgentType) ReflectedAgentType {
	conv, err := witschema.GraphToCore(wit.Schema)
	return ReflectedAgentType{wit: wit, conv: conv, convErr: err}
}

// Name returns the agent type's name.
func (r ReflectedAgentType) Name() string { return r.wit.TypeName }

// Description returns the agent type's documentation.
func (r ReflectedAgentType) Description() string { return r.wit.Description }

// SourceLanguage returns the language the agent type was written in.
func (r ReflectedAgentType) SourceLanguage() string { return r.wit.SourceLanguage }

// Schema returns the snapshot's type graph. Its root is a placeholder: the
// meaningful roots are the per-parameter and per-output nodes.
func (r ReflectedAgentType) Schema() (core.Ref, error) {
	if r.convErr != nil {
		return core.Ref{}, r.convErr
	}
	return core.NewRef(r.conv.Graph), nil
}

// Constructor returns the agent type's constructor.
func (r ReflectedAgentType) Constructor() ReflectedConstructor {
	return ReflectedConstructor{conv: r.conv, convErr: r.convErr, wit: r.wit.Constructor}
}

// Methods returns the agent type's methods in declaration order.
func (r ReflectedAgentType) Methods() []ReflectedMethod {
	out := make([]ReflectedMethod, 0, len(r.wit.Methods))
	for _, m := range r.wit.Methods {
		out = append(out, ReflectedMethod{conv: r.conv, convErr: r.convErr, wit: m})
	}
	return out
}

// Method looks one method up by name.
func (r ReflectedAgentType) Method(name string) (ReflectedMethod, bool) {
	for _, m := range r.wit.Methods {
		if m.Name == name {
			return ReflectedMethod{conv: r.conv, convErr: r.convErr, wit: m}, true
		}
	}
	return ReflectedMethod{}, false
}

// ReflectedConstructor is a snapshot of an agent type's constructor.
type ReflectedConstructor struct {
	conv    witschema.Converted
	convErr error
	wit     common.AgentConstructor
}

// Description returns the constructor's documentation.
func (c ReflectedConstructor) Description() string { return c.wit.Description }

// Parameters returns the constructor's caller-supplied parameters. Fields the
// host injects, such as the principal, are left out: a caller neither supplies
// nor can override them.
func (c ReflectedConstructor) Parameters() ([]core.Parameter, error) {
	return userParameters(c.conv, c.convErr, c.wit.InputSchema)
}

// PackJSON builds the constructor's parameter value from named arguments,
// validating each against the snapshot before anything is sent.
func (c ReflectedConstructor) PackJSON(args map[string]any) (core.SchemaValue, error) {
	params, err := c.Parameters()
	if err != nil {
		return nil, err
	}
	return core.NewRef(c.conv.Graph).PackParameters(params, args)
}

//nolint:unused // called from reflection_wasm.go
func (c ReflectedConstructor) packTree(args map[string]any) (types.SchemaValueTree, error) {
	built, err := c.PackJSON(args)
	if err != nil {
		return types.SchemaValueTree{}, err
	}
	return witschema.ValueToWit(built)
}

// ToJSONSchema renders the constructor's parameters as a JSON Schema object.
func (c ReflectedConstructor) ToJSONSchema(includeDraftMarker bool) (any, error) {
	params, err := c.Parameters()
	if err != nil {
		return nil, err
	}
	return core.NewRef(c.conv.Graph).ParametersJSONSchema(params, includeDraftMarker)
}

// ReflectedMethod is a snapshot of one agent method.
type ReflectedMethod struct {
	conv    witschema.Converted
	convErr error
	wit     common.AgentMethod
}

// Name returns the method's name.
func (m ReflectedMethod) Name() string { return m.wit.Name }

// Description returns the method's documentation.
func (m ReflectedMethod) Description() string { return m.wit.Description }

// PromptHint returns the hint offered to a model choosing this method, if any.
func (m ReflectedMethod) PromptHint() (string, bool) {
	if m.wit.PromptHint.IsNone() {
		return "", false
	}
	return m.wit.PromptHint.Some(), true
}

// ReadOnly reports whether the method declares itself free of observable
// effects, which is what makes its result cacheable.
func (m ReflectedMethod) ReadOnly() bool { return m.wit.ReadOnly.IsSome() }

// Parameters returns the method's caller-supplied parameters.
func (m ReflectedMethod) Parameters() ([]core.Parameter, error) {
	return userParameters(m.conv, m.convErr, m.wit.InputSchema)
}

// Output returns the method's result type, or false when it returns nothing.
func (m ReflectedMethod) Output() (core.Ref, bool) {
	if m.wit.OutputSchema.Tag() != common.OutputSchemaSingle || m.convErr != nil {
		return core.Ref{}, false
	}
	ref, err := m.conv.Ref(m.wit.OutputSchema.Single())
	if err != nil {
		return core.Ref{}, false
	}
	return ref, true
}

// PackJSON builds the method's parameter value from named arguments.
func (m ReflectedMethod) PackJSON(args map[string]any) (core.SchemaValue, error) {
	params, err := m.Parameters()
	if err != nil {
		return nil, err
	}
	return core.NewRef(m.conv.Graph).PackParameters(params, args)
}

func (m ReflectedMethod) packTree(args map[string]any) (types.SchemaValueTree, error) {
	built, err := m.PackJSON(args)
	if err != nil {
		return types.SchemaValueTree{}, err
	}
	return witschema.ValueToWit(built)
}

// UnpackOutput reads a returned value as canonical JSON.
func (m ReflectedMethod) UnpackOutput(value core.SchemaValue) (any, error) {
	out, has := m.Output()
	if !has {
		return nil, fmt.Errorf("golem: method %q returns nothing", m.wit.Name)
	}
	return out.UnpackJSON(value)
}

func (m ReflectedMethod) unpackTree(tree types.SchemaValueTree) (any, error) {
	value, err := witschema.ValueToCore(tree)
	if err != nil {
		return nil, err
	}
	return m.UnpackOutput(value)
}

// ToJSONSchema renders the method's parameters as a JSON Schema object.
func (m ReflectedMethod) ToJSONSchema(includeDraftMarker bool) (any, error) {
	params, err := m.Parameters()
	if err != nil {
		return nil, err
	}
	return core.NewRef(m.conv.Graph).ParametersJSONSchema(params, includeDraftMarker)
}

// OutputJSONSchema renders the method's result type, or false when it returns
// nothing.
func (m ReflectedMethod) OutputJSONSchema(includeDraftMarker bool) (any, bool, error) {
	out, has := m.Output()
	if !has {
		return nil, false, nil
	}
	rendered, err := out.ToJSONSchema(includeDraftMarker)
	return rendered, true, err
}

// userParameters keeps the fields a caller supplies. An auto-injected field —
// the principal, today — is filled in by the host, so asking a caller for it
// would be wrong twice over: it cannot know the value, and supplying one would
// not be honoured.
func userParameters(conv witschema.Converted, convErr error, in common.InputSchema) ([]core.Parameter, error) {
	if convErr != nil {
		return nil, convErr
	}
	fields := in.Parameters()
	out := make([]core.Parameter, 0, len(fields))
	for _, f := range fields {
		if f.Source.Tag() != common.FieldSourceUserSupplied {
			continue
		}
		t, err := conv.At(f.Schema)
		if err != nil {
			return nil, fmt.Errorf("golem: parameter %q: %w", f.Name, err)
		}
		out = append(out, core.Parameter{Name: f.Name, Type: t})
	}
	return out, nil
}

// ReflectedAgentClient invokes a discovered agent. Every call is packed and
// validated against the snapshot the client was built from, so a caller that
// never saw the target's types still cannot send it a malformed argument.
type ReflectedAgentClient struct {
	agentType ReflectedAgentType
	agentID   string
	rpc       reflectedRPC
}

// reflectedRPC is the host connection a reflected client invokes through. The
// interface keeps packing, validation and decoding testable without a host.
type reflectedRPC interface {
	invokeAndAwait(method string, input types.SchemaValueTree) (types.SchemaValueTree, bool, error)
}

// AgentType returns the snapshot this client was built from.
func (c *ReflectedAgentClient) AgentType() ReflectedAgentType { return c.agentType }

// AgentID returns the target's agent id, as resolved by the host.
func (c *ReflectedAgentClient) AgentID() string { return c.agentID }

// InvokeAndAwait calls a method with named arguments and returns its result as
// canonical JSON, or nil when the method returns nothing.
func (c *ReflectedAgentClient) InvokeAndAwait(method string, args map[string]any) (any, error) {
	m, known := c.agentType.Method(method)
	if !known {
		return nil, fmt.Errorf("golem: agent type %q has no method %q", c.agentType.Name(), method)
	}
	input, err := m.packTree(args)
	if err != nil {
		return nil, fmt.Errorf("golem: %s.%s: %w", c.agentType.Name(), method, err)
	}
	tree, has, err := c.rpc.invokeAndAwait(method, input)
	if err != nil {
		return nil, err
	}

	_, declared := m.Output()
	switch {
	case has && !declared:
		// Cardinality is part of the contract, so an unexpected value is a
		// remote output error rather than something to quietly drop.
		return nil, fmt.Errorf("golem: %s.%s returned a value but declares none",
			c.agentType.Name(), method)
	case !has && declared:
		return nil, fmt.Errorf("golem: %s.%s returned nothing but declares a result",
			c.agentType.Name(), method)
	case !has:
		return nil, nil
	}
	out, err := m.unpackTree(tree)
	if err != nil {
		return nil, fmt.Errorf("golem: %s.%s returned an unreadable result: %w",
			c.agentType.Name(), method, err)
	}
	return out, nil
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

// Arguments returns the command's arguments as a parameter list, in the order
// and with the types of its canonical input record.
func (c ReflectedCommand) Arguments() ([]core.Parameter, error) {
	if c.convErr != nil {
		return nil, c.convErr
	}
	fields, err := c.canonicalFields()
	if err != nil {
		return nil, err
	}
	out := make([]core.Parameter, 0, len(fields))
	for _, f := range fields {
		switch f.wrap {
		case wrapBool:
			out = append(out, core.BoolParameter(f.name))
			continue
		case wrapCount:
			out = append(out, core.Parameter{Name: f.name, Type: core.SchemaType{Body: core.U32Type{}}})
			continue
		}
		t, err := c.conv.At(f.node)
		if err != nil {
			return nil, fmt.Errorf("golem: argument %q: %w", f.name, err)
		}
		switch f.wrap {
		case wrapOption:
			t = core.SchemaType{Body: core.OptionType{Inner: t}}
		case wrapList:
			t = core.SchemaType{Body: core.ListType{Element: t}}
		}
		out = append(out, core.Parameter{Name: f.name, Type: t})
	}
	return out, nil
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

// Result returns the command's result type, or false when it produces none.
func (c ReflectedCommand) Result() (core.Ref, bool) {
	if !c.Callable() || c.convErr != nil {
		return core.Ref{}, false
	}
	body := c.node().Body.Some()
	if body.Result.IsNone() {
		return core.Ref{}, false
	}
	ref, err := c.conv.Ref(body.Result.Some().Type)
	if err != nil {
		return core.Ref{}, false
	}
	return ref, true
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

// PackJSON builds the command's invocation input from named arguments, one per
// field of its canonical input record.
func (c ReflectedCommand) PackJSON(args map[string]any) (TypedValue, error) {
	params, err := c.Arguments()
	if err != nil {
		return TypedValue{}, err
	}
	fields, err := c.canonicalFields()
	if err != nil {
		return TypedValue{}, err
	}
	built, err := core.NewRef(c.conv.Graph).PackParameters(params, args)
	if err != nil {
		return TypedValue{}, err
	}
	tree, err := witschema.ValueToWit(built)
	if err != nil {
		return TypedValue{}, err
	}
	return TypedValue{wit: types.TypedSchemaValue{Graph: c.inputGraph(fields), Value: tree}}, nil
}

// ToJSONSchema renders the command's arguments as a JSON Schema object.
func (c ReflectedCommand) ToJSONSchema(includeDraftMarker bool) (any, error) {
	params, err := c.Arguments()
	if err != nil {
		return nil, err
	}
	return core.NewRef(c.conv.Graph).ParametersJSONSchema(params, includeDraftMarker)
}

// ReflectedToolClient invokes a discovered tool. Arguments are packed and
// validated against the snapshot before anything is sent.
type ReflectedToolClient struct {
	tool ReflectedTool
	rpc  reflectedToolRPC
}

// reflectedToolRPC is the host connection a reflected tool client invokes
// through.
type reflectedToolRPC interface {
	invokeAndAwait(commandPath []string, input types.TypedSchemaValue) (types.TypedSchemaValue, bool, error)
}

// Tool returns the snapshot this client was built from.
func (c *ReflectedToolClient) Tool() ReflectedTool { return c.tool }

// InvokeAndAwait runs a command with named arguments and returns its result as
// canonical JSON, or nil when the command produces none.
func (c *ReflectedToolClient) InvokeAndAwait(path []string, args map[string]any) (any, error) {
	cmd, found := c.tool.Command(path)
	if !found {
		return nil, fmt.Errorf("golem: tool %q has no command %s", c.tool.Name(), commandLabel(path))
	}
	if !cmd.Callable() {
		// A namespace node stays discoverable so a caller can walk to its
		// children, but it has nothing to run.
		return nil, fmt.Errorf("golem: tool %q command %s only dispatches to subcommands",
			c.tool.Name(), commandLabel(path))
	}
	input, err := cmd.PackJSON(args)
	if err != nil {
		return nil, fmt.Errorf("golem: %s %s: %w", c.tool.Name(), commandLabel(path), err)
	}

	out, has, err := c.rpc.invokeAndAwait(path, input.wit)
	if err != nil {
		return nil, err
	}
	_, declared := cmd.Result()
	switch {
	case has && !declared:
		return nil, fmt.Errorf("golem: %s %s returned a value but declares none",
			c.tool.Name(), commandLabel(path))
	case !has && declared:
		return nil, fmt.Errorf("golem: %s %s returned nothing but declares a result",
			c.tool.Name(), commandLabel(path))
	case !has:
		return nil, nil
	}
	value, err := TypedValue{wit: out}.JSON()
	if err != nil {
		return nil, fmt.Errorf("golem: %s %s returned an unreadable result: %w",
			c.tool.Name(), commandLabel(path), err)
	}
	return value, nil
}

// toolRPCErrorMessage renders the host's tool RPC failure. The remote tool's
// own error stays structured — it is rendered through the same helper the
// middleware layer uses — so a caller can still tell a denial from the tool
// having reported a declared failure.
//
//nolint:unused // called from reflection_wasm.go
func toolRPCErrorMessage(e types.ToolRpcError) string {
	switch e.Tag() {
	case types.ToolRpcErrorProtocolError:
		return "protocol error: " + e.ProtocolError()
	case types.ToolRpcErrorDenied:
		return "denied: " + e.Denied()
	case types.ToolRpcErrorNotFound:
		return "not found"
	case types.ToolRpcErrorRemoteInternalError:
		return "remote internal error: " + e.RemoteInternalError()
	case types.ToolRpcErrorRemoteToolError:
		return "the tool reported: " + toolErrorMessage(e.RemoteToolError())
	case types.ToolRpcErrorCancelled:
		return "cancelled"
	case types.ToolRpcErrorResourceExhausted:
		return "resource exhausted: " + e.ResourceExhausted()
	}
	return "failed"
}
