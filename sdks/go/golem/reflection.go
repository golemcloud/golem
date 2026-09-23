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

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	"github.com/golemcloud/golem/sdks/go/golem/schema"
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
}

// Name returns the agent type's name.
func (r ReflectedAgentType) Name() string { return r.wit.TypeName }

// Description returns the agent type's documentation.
func (r ReflectedAgentType) Description() string { return r.wit.Description }

// SourceLanguage returns the language the agent type was written in.
func (r ReflectedAgentType) SourceLanguage() string { return r.wit.SourceLanguage }

// Schema returns the snapshot's type graph. Its root is a placeholder: the
// meaningful roots are the per-parameter and per-output nodes.
func (r ReflectedAgentType) Schema() schema.Ref { return schema.NewRef(r.wit.Schema) }

// Constructor returns the agent type's constructor.
func (r ReflectedAgentType) Constructor() ReflectedConstructor {
	return ReflectedConstructor{graph: r.wit.Schema, wit: r.wit.Constructor}
}

// Methods returns the agent type's methods in declaration order.
func (r ReflectedAgentType) Methods() []ReflectedMethod {
	out := make([]ReflectedMethod, 0, len(r.wit.Methods))
	for _, m := range r.wit.Methods {
		out = append(out, ReflectedMethod{graph: r.wit.Schema, wit: m})
	}
	return out
}

// Method looks one method up by name.
func (r ReflectedAgentType) Method(name string) (ReflectedMethod, bool) {
	for _, m := range r.wit.Methods {
		if m.Name == name {
			return ReflectedMethod{graph: r.wit.Schema, wit: m}, true
		}
	}
	return ReflectedMethod{}, false
}

// ReflectedConstructor is a snapshot of an agent type's constructor.
type ReflectedConstructor struct {
	graph types.SchemaGraph
	wit   common.AgentConstructor
}

// Description returns the constructor's documentation.
func (c ReflectedConstructor) Description() string { return c.wit.Description }

// Parameters returns the constructor's caller-supplied parameters. Fields the
// host injects, such as the principal, are left out: a caller neither supplies
// nor can override them.
func (c ReflectedConstructor) Parameters() []schema.Parameter {
	return userParameters(c.wit.InputSchema)
}

// PackJSON builds the constructor's value tree from named arguments, validating
// each against the snapshot before anything is sent.
func (c ReflectedConstructor) PackJSON(args map[string]any) (types.SchemaValueTree, error) {
	return schema.NewRef(c.graph).PackParameters(c.Parameters(), args)
}

// ToJSONSchema renders the constructor's parameters as a JSON Schema object.
func (c ReflectedConstructor) ToJSONSchema(includeDraftMarker bool) (any, error) {
	return schema.NewRef(c.graph).ParametersJSONSchema(c.Parameters(), includeDraftMarker)
}

// ReflectedMethod is a snapshot of one agent method.
type ReflectedMethod struct {
	graph types.SchemaGraph
	wit   common.AgentMethod
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
func (m ReflectedMethod) Parameters() []schema.Parameter {
	return userParameters(m.wit.InputSchema)
}

// Output returns the method's result type, or false when it returns nothing.
func (m ReflectedMethod) Output() (schema.Ref, bool) {
	if m.wit.OutputSchema.Tag() != common.OutputSchemaSingle {
		return schema.Ref{}, false
	}
	return schema.NewRef(m.graph).WithRoot(m.wit.OutputSchema.Single()), true
}

// PackJSON builds the method's input tree from named arguments.
func (m ReflectedMethod) PackJSON(args map[string]any) (types.SchemaValueTree, error) {
	return schema.NewRef(m.graph).PackParameters(m.Parameters(), args)
}

// UnpackOutput reads a returned value tree as canonical JSON.
func (m ReflectedMethod) UnpackOutput(tree types.SchemaValueTree) (any, error) {
	out, has := m.Output()
	if !has {
		return nil, fmt.Errorf("golem: method %q returns nothing", m.wit.Name)
	}
	return out.UnpackJSON(tree)
}

// ToJSONSchema renders the method's parameters as a JSON Schema object.
func (m ReflectedMethod) ToJSONSchema(includeDraftMarker bool) (any, error) {
	return schema.NewRef(m.graph).ParametersJSONSchema(m.Parameters(), includeDraftMarker)
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
func userParameters(in common.InputSchema) []schema.Parameter {
	fields := in.Parameters()
	out := make([]schema.Parameter, 0, len(fields))
	for _, f := range fields {
		if f.Source.Tag() != common.FieldSourceUserSupplied {
			continue
		}
		out = append(out, schema.Parameter{Name: f.Name, Node: f.Schema})
	}
	return out
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
	input, err := m.PackJSON(args)
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
	out, err := m.UnpackOutput(tree)
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
}

// Name returns the name a client binds to, which is stable across adapters.
func (r ReflectedTool) Name() string { return r.lookupName }

// Version returns the tool's own version.
func (r ReflectedTool) Version() string { return r.wit.Version }

// Schema returns the tool's type pool. Its root is a placeholder: command
// bodies index into it.
func (r ReflectedTool) Schema() schema.Ref { return schema.NewRef(r.wit.Schema) }

// Root returns the tool's root command.
func (r ReflectedTool) Root() ReflectedCommand {
	return ReflectedCommand{graph: r.wit.Schema, tree: r.wit.Commands, index: 0}
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
	graph types.SchemaGraph
	tree  toolCommon.CommandTree
	index int32
	path  []string
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
			graph: c.graph, tree: c.tree, index: idx,
			path: append(append([]string(nil), c.path...), c.tree.Nodes[idx].Name),
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

// Arguments returns the command's arguments as a parameter list, with
// positionals first, then options, then flags — the order the invocation record
// carries them in.
func (c ReflectedCommand) Arguments() ([]schema.Parameter, error) {
	if !c.Callable() {
		return nil, fmt.Errorf("golem: command %q has no body", c.Name())
	}
	body := c.node().Body.Some()
	out := make([]schema.Parameter, 0,
		len(body.Positionals.Fixed)+len(body.Options)+len(body.Flags))
	for _, p := range body.Positionals.Fixed {
		out = append(out, schema.Parameter{Name: p.Name, Node: p.Type})
	}
	for _, o := range body.Options {
		node, err := optionValueNode(o.Shape)
		if err != nil {
			return nil, fmt.Errorf("golem: option %q: %w", o.Long, err)
		}
		out = append(out, schema.Parameter{Name: o.Long, Node: node})
	}
	for _, f := range body.Flags {
		// A flag has no type index: the WIT fixes its type as bool, so the
		// graph need not name one.
		out = append(out, schema.BoolParameter(f.Long))
	}
	return out, nil
}

// Result returns the command's result type, or false when it produces none.
func (c ReflectedCommand) Result() (schema.Ref, bool) {
	if !c.Callable() {
		return schema.Ref{}, false
	}
	body := c.node().Body.Some()
	if body.Result.IsNone() {
		return schema.Ref{}, false
	}
	return schema.NewRef(c.graph).WithRoot(body.Result.Some().Type), true
}

// Errors returns the failures the command declares.
func (c ReflectedCommand) Errors() []toolCommon.ErrorCase {
	if !c.Callable() {
		return nil
	}
	return c.node().Body.Some().Errors
}

// PackJSON builds the command's invocation input from named arguments.
func (c ReflectedCommand) PackJSON(args map[string]any) (types.TypedSchemaValue, error) {
	params, err := c.Arguments()
	if err != nil {
		return types.TypedSchemaValue{}, err
	}
	tree, err := schema.NewRef(c.graph).PackParameters(params, args)
	if err != nil {
		return types.TypedSchemaValue{}, err
	}
	return types.TypedSchemaValue{Graph: c.graph, Value: tree}, nil
}

// ToJSONSchema renders the command's arguments as a JSON Schema object.
func (c ReflectedCommand) ToJSONSchema(includeDraftMarker bool) (any, error) {
	params, err := c.Arguments()
	if err != nil {
		return nil, err
	}
	return schema.NewRef(c.graph).ParametersJSONSchema(params, includeDraftMarker)
}

// optionValueNode reports the type node an option's value is read against,
// whichever shape the option takes.
func optionValueNode(shape toolCommon.OptionShape) (int32, error) {
	switch shape.Tag() {
	case toolCommon.OptionShapeScalar:
		return shape.Scalar(), nil
	case toolCommon.OptionShapeOptionalScalar:
		return shape.OptionalScalar(), nil
	case toolCommon.OptionShapeRepeatableList:
		// The collected value is a list, but the metadata names its element
		// type, so the two are not interchangeable here.
		return 0, fmt.Errorf("a repeatable option collects into a list, which the tool schema does not name")
	case toolCommon.OptionShapeRepeatableMap:
		return shape.RepeatableMap().MapType, nil
	}
	return 0, fmt.Errorf("unknown option shape (tag %d)", shape.Tag())
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

	out, has, err := c.rpc.invokeAndAwait(path, input)
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
