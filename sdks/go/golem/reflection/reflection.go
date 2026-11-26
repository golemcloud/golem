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

// Package reflection discovers the agent types and tools deployed in the
// caller's environment and calls them without sharing their Go definitions:
// reflected clients validate canonical JSON against a discovered snapshot, and
// dynamic clients send values the caller packed itself.
package reflection

import (
	"errors"
	"fmt"
	"slices"
	"strings"
	"time"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	"github.com/golemcloud/golem/sdks/go/golem/internal/witschema"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Reflection.
//
// A reflected client is built from what the deployment says, not from a shared
// Go definition: discovery returns an immutable snapshot of an agent type, and
// that snapshot validates and packs every call made through it.
//
//	agentType, found := reflection.DiscoverAgentType("Greeter")
//	if !found { ... }
//	client, err := agentType.Get(map[string]any{"name": "ada"})
//	result, metadata, err := client.Call("greet", map[string]any{"greeting": "hi"})
//	invocation, err := client.Trigger("greet", map[string]any{"greeting": "hi"})
//
// Arguments and results are canonical JSON — ordinary Go values read against
// the snapshot's schema — because the caller has none of the target's types.
// Snapshots never refresh themselves; discover again when a newer deployment
// matters.

// AgentType is an immutable snapshot of a deployed agent type.
type AgentType struct {
	wit common.AgentType
	// conv is the snapshot's schema in the shared model, converted once when
	// the snapshot is taken rather than on every call. It also carries the
	// index side table, because the wire selects sub-schemas by node index and
	// the shared model has no indices.
	conv        witschema.Converted
	constructor Constructor
	methods     []Method
}

// newAgentType converts a discovered agent type's schema, and the constructor's
// and methods' signatures within it, once, when the snapshot is taken. The host
// only describes well-formed agent types, so a graph that does not convert is a
// broken host and panics.
func newAgentType(wit common.AgentType) AgentType {
	conv, err := witschema.GraphToCore(wit.Schema)
	if err != nil {
		panic(fmt.Errorf("golem: agent type %q has a malformed schema: %w", wit.TypeName, err))
	}
	ctorInput, err := parametersRecord(conv, wit.Constructor.InputSchema)
	if err != nil {
		panic(fmt.Errorf("golem: agent type %q constructor: %w", wit.TypeName, err))
	}
	methods := make([]Method, 0, len(wit.Methods))
	for _, m := range wit.Methods {
		input, err := parametersRecord(conv, m.InputSchema)
		if err != nil {
			panic(fmt.Errorf("golem: agent type %q method %q: %w", wit.TypeName, m.Name, err))
		}
		streaming := false
		for _, f := range m.InputSchema.Parameters() {
			streaming = streaming || engine.GraphCarriesStream(wit.Schema, f.Schema)
		}
		output := golem.None[core.Ref]()
		if m.OutputSchema.Tag() == common.OutputSchemaSingle {
			streaming = streaming || engine.GraphCarriesStream(wit.Schema, m.OutputSchema.Single())
			ref, err := conv.Ref(m.OutputSchema.Single())
			if err != nil {
				panic(fmt.Errorf("golem: agent type %q method %q result: %w", wit.TypeName, m.Name, err))
			}
			output = golem.Some(ref)
		}
		methods = append(methods, Method{wit: m, input: input, output: output, streaming: streaming})
	}
	return AgentType{
		wit:         wit,
		conv:        conv,
		constructor: Constructor{wit: wit.Constructor, input: ctorInput},
		methods:     methods,
	}
}

// Name returns the agent type's name.
func (r AgentType) Name() string { return r.wit.TypeName }

// Description returns the agent type's documentation.
func (r AgentType) Description() string { return r.wit.Description }

// SourceLanguage returns the language the agent type was written in.
func (r AgentType) SourceLanguage() string { return r.wit.SourceLanguage }

// Mode returns the agent type's lifecycle.
func (r AgentType) Mode() golem.Mode {
	if r.wit.Mode == common.AgentModeEphemeral {
		return golem.Ephemeral
	}
	return golem.Durable
}

// Schema returns the snapshot's type graph. Its root is a placeholder: the
// meaningful roots are the constructor's and the methods' inputs and outputs.
func (r AgentType) Schema() core.Ref {
	return core.NewRef(r.conv.Graph)
}

// Constructor returns the agent type's constructor.
func (r AgentType) Constructor() Constructor { return r.constructor }

// Methods returns the agent type's methods in declaration order.
func (r AgentType) Methods() []Method { return slices.Clone(r.methods) }

// Method looks one method up by name.
func (r AgentType) Method(name string) (Method, bool) {
	for _, m := range r.methods {
		if m.wit.Name == name {
			return m, true
		}
	}
	return Method{}, false
}

// packConstructor validates and packs constructor arguments.
//
//nolint:unused // called from host_wasm.go
func (r AgentType) packConstructor(args map[string]any) (types.SchemaValueTree, error) {
	return packJSONTree(r.constructor.input, args)
}

// Constructor is a snapshot of an agent type's constructor.
type Constructor struct {
	wit   common.AgentConstructor
	input core.Ref
}

// Description returns the constructor's documentation.
func (c Constructor) Description() string { return c.wit.Description }

// Input returns the constructor's parameters as a record type. Fields the host
// injects, such as the principal, are left out: a caller neither supplies nor
// can override them. Pack canonical JSON with its PackJSON, and render it with
// ToJSONSchema.
func (c Constructor) Input() core.Ref { return c.input }

// Method is a snapshot of one agent method.
type Method struct {
	wit       common.AgentMethod
	input     core.Ref
	output    golem.Option[core.Ref]
	streaming bool
}

// Name returns the method's name.
func (m Method) Name() string { return m.wit.Name }

// Description returns the method's documentation.
func (m Method) Description() string { return m.wit.Description }

// PromptHint returns the hint offered to a model choosing this method, if any.
func (m Method) PromptHint() (string, bool) {
	if m.wit.PromptHint.IsNone() {
		return "", false
	}
	return m.wit.PromptHint.Some(), true
}

// ReadOnly reports whether the method declares itself free of observable
// effects, which is what makes its result cacheable.
func (m Method) ReadOnly() bool { return m.wit.ReadOnly.IsSome() }

// Input returns the method's caller-supplied parameters as a record type.
func (m Method) Input() core.Ref { return m.input }

// Output returns the method's result type, none when it returns nothing.
func (m Method) Output() golem.Option[core.Ref] { return m.output }

// parametersRecord is a parameter list as one record type: the caller-supplied
// fields, in order. An auto-injected field — the principal, today — is filled
// in by the host, so asking a caller for it would be wrong twice over: it
// cannot know the value, and supplying one would not be honoured.
func parametersRecord(conv witschema.Converted, in common.InputSchema) (core.Ref, error) {
	params := in.Parameters()
	fields := make([]core.NamedField, 0, len(params))
	for _, f := range params {
		if f.Source.Tag() != common.FieldSourceUserSupplied {
			continue
		}
		t, err := conv.At(f.Schema)
		if err != nil {
			return core.Ref{}, fmt.Errorf("golem: parameter %q: %w", f.Name, err)
		}
		fields = append(fields, core.NamedField{Name: f.Name, Body: t})
	}
	return core.NewRefAt(conv.Graph, core.SchemaType{Body: core.RecordType{Fields: fields}}), nil
}

// configValues checks untyped configuration entries against the snapshot's
// declarations and renders them for the wire. Every problem is reported, not
// just the first.
func (r AgentType) configValues(entries []configOverride) ([]common.TypedAgentConfigValue, error) {
	if len(entries) == 0 {
		return nil, nil
	}
	var problems []error
	out := make([]common.TypedAgentConfigValue, 0, len(entries))
	for _, e := range entries {
		label := strings.Join(e.path, ".")
		idx := slices.IndexFunc(r.wit.Config, func(d common.AgentConfigDeclaration) bool { return slices.Equal(d.Path, e.path) })
		if idx < 0 {
			problems = append(problems, fmt.Errorf("%s is not a declared configuration path", label))
			continue
		}
		decl := r.wit.Config[idx]
		if decl.Source == common.AgentConfigSourceSecret {
			problems = append(problems, fmt.Errorf("%s is a secret, which the platform provisions", label))
			continue
		}
		ref, err := r.conv.Ref(decl.ValueType)
		value := e.value
		if err == nil {
			if e.native {
				_, err = ref.UnpackJSON(value)
			} else {
				value, err = ref.PackJSON(e.json)
			}
		}
		var tree types.SchemaValueTree
		if err == nil {
			tree, err = witschema.ValueToWit(value)
		}
		if err != nil {
			problems = append(problems, fmt.Errorf("%s: %w", label, err))
			continue
		}
		graph := r.wit.Schema
		graph.Root = decl.ValueType
		out = append(out, common.TypedAgentConfigValue{
			Path: slices.Clone(e.path), Value: types.TypedSchemaValue{Graph: graph, Value: tree},
		})
	}
	if len(problems) > 0 {
		return nil, fmt.Errorf("golem: %s configuration: %w", r.Name(), errors.Join(problems...))
	}
	return out, nil
}

// packJSONTree validates canonical JSON against a type and renders it for the
// wire.
func packJSONTree(ref core.Ref, value any) (types.SchemaValueTree, error) {
	built, err := ref.PackJSON(value)
	if err != nil {
		return types.SchemaValueTree{}, err
	}
	return witschema.ValueToWit(built)
}

// AgentClient invokes a discovered agent. Every call is packed and
// validated against the snapshot the client was built from, so a caller that
// never saw the target's types still cannot send it a malformed argument.
type AgentClient struct {
	agentType AgentType
	// id is shared by the copies of a client, so the ID that AgentID learns
	// from the host is learned once.
	id        *agentIDCell
	rpc       agentRPC
	scopeCard any // a golem.ScopeCard, or nil
}

type agentIDCell struct {
	id    golem.AgentID
	known bool
}

func knownAgentID(id golem.AgentID) *agentIDCell { return &agentIDCell{id: id, known: true} }

// WithScopeCard returns a copy of the client whose awaited calls carry card as
// their scope card, unless the call passes its own with [CallScopeCard], as
// [golem.Client.WithScopeCard] does for a typed client. Trigger and Schedule
// never carry one. The card is lent, not moved,
// and is valid only in the invocation that derived it. It panics on the zero
// ScopeCard.
func (c *AgentClient) WithScopeCard(card golem.ScopeCard) *AgentClient {
	scoped := *c
	scoped.scopeCard = mustScopeCard(c.agentType.Name(), card)
	return &scoped
}

// agentRPC is the host connection a reflected or dynamic agent client invokes
// through. The interface keeps packing, validation and decoding testable
// without a host.
type agentRPC interface {
	// call makes an awaited call, and returns its result with the
	// invocation's identity.
	call(method string, input types.SchemaValueTree, scopeCard witTypes.Option[*types.PermissionCard]) (witTypes.Option[types.SchemaValueTree], golem.InvocationMetadata, error)
	trigger(method string, input types.SchemaValueTree) (link.InvocationOps, error)
	schedule(at time.Time, method string, input types.SchemaValueTree) (link.InvocationOps, error)
	// agentID asks the host for the ID of the agent, which creates an agent
	// that an upsert addresses when it is missing.
	agentID() (golem.AgentID, error)
}

// AgentType returns the snapshot this client was built from.
func (c *AgentClient) AgentType() AgentType { return c.agentType }

// AgentID returns the ID of the client's agent. A client of Upsert asks the
// host for it the first time, and the host creates the agent when it is
// missing; the other clients already know it. A failure is a
// [golem.RemoteCallError].
func (c *AgentClient) AgentID() (golem.AgentID, error) {
	if c.id.known {
		return c.id.id, nil
	}
	id, err := c.rpc.agentID()
	if err != nil {
		return golem.AgentID{}, err
	}
	c.id.id, c.id.known = id, true
	return id, nil
}

// target names the client's agent in errors: its ID when the client knows it,
// and otherwise its agent type.
func (c *AgentClient) target() string {
	if c.id != nil && c.id.known {
		return c.id.id.String()
	}
	return c.agentType.Name()
}

func (c *AgentClient) pack(method string, args map[string]any) (Method, types.SchemaValueTree, error) {
	m, known := c.agentType.Method(method)
	if !known {
		return m, types.SchemaValueTree{}, fmt.Errorf("golem: agent type %q has no method %q", c.agentType.Name(), method)
	}
	tree, err := packJSONTree(m.Input(), args)
	if err == nil {
		return m, tree, nil
	}
	return m, types.SchemaValueTree{}, fmt.Errorf("golem: %s.%s: %w", c.agentType.Name(), method, err)
}

// packNotAwaited packs the arguments of a Trigger or a Schedule, which return
// before the call completes and so cannot hand the caller a stream endpoint.
func (c *AgentClient) packNotAwaited(form, method string, args map[string]any) (Method, types.SchemaValueTree, error) {
	m, input, err := c.pack(method, args)
	if err == nil && m.streaming {
		err = fmt.Errorf("golem: %s.%s: %s cannot carry a stream; await the call with Call instead", c.agentType.Name(), method, form)
	}
	return m, input, err
}

// Call invokes a method with named arguments and waits for its result, as
// canonical JSON (nil when the method returns nothing), together with the
// invocation's identity. A call that the target withdrew before it started
// fails with a [golem.RemoteCallError] of kind [golem.RemoteCancelled].
//
// An awaited call cannot be withdrawn by abandoning it: the goroutine that
// makes it waits for its result. Use Trigger and [golem.Invocation.Cancel] for
// work that may need to be withdrawn before it starts.
func (c *AgentClient) Call(method string, args map[string]any, opts ...CallOpt) (any, golem.InvocationMetadata, error) {
	scope := scopeCardOf(c.scopeCard, opts)
	m, input, err := c.pack(method, args)
	if err != nil {
		return nil, golem.InvocationMetadata{}, err
	}
	res, metadata, err := c.rpc.call(method, input, scope)
	if err != nil {
		return nil, golem.InvocationMetadata{}, err
	}
	out, err := c.decodeResult(m, method, res)
	if err != nil {
		return nil, golem.InvocationMetadata{}, err
	}
	return out, metadata, nil
}

// decodeResult reads a method's result against the snapshot.
func (c *AgentClient) decodeResult(m Method, method string, res witTypes.Option[types.SchemaValueTree]) (any, error) {
	out, declared := m.Output().Get()
	tree, has := optionFromWit(res).Get()
	switch {
	case has && !declared:
		// Cardinality is part of the contract, so an unexpected value is a
		// remote output error rather than something to quietly drop.
		return nil, c.protocolError(method, "remote returned a value but the method declares none")
	case !has && declared:
		return nil, c.protocolError(method, "remote returned no value but the method declares a result")
	case !has:
		return nil, nil
	}
	value, err := witschema.ValueToCore(tree)
	if err == nil {
		var unpacked any
		unpacked, err = out.UnpackJSON(value)
		if err == nil {
			return unpacked, nil
		}
	}
	return nil, c.protocolError(method, "remote returned an undecodable value: "+err.Error())
}

// protocolError is a result the call cannot read, reported as typed calls
// report it.
func (c *AgentClient) protocolError(method, message string) error {
	return &golem.RemoteCallError{Target: c.target(), Method: method, Kind: golem.RemoteProtocolError, Message: message}
}

// Trigger starts a method with named arguments without waiting for its result,
// and returns the handle of the invocation once the target has durably accepted
// it. The handle's AwaitResult reads the result as canonical JSON, as Call
// does. A Trigger never carries a scope card, and refuses a method that takes
// or returns a stream.
func (c *AgentClient) Trigger(method string, args map[string]any) (*golem.Invocation[any], error) {
	m, input, err := c.packNotAwaited("Trigger", method, args)
	if err != nil {
		return nil, err
	}
	ops, err := c.rpc.trigger(method, input)
	if err != nil {
		return nil, err
	}
	return c.invocation(m, method, ops), nil
}

// Schedule arranges for a method to be invoked at the given time, and returns
// the handle of the invocation once the target has durably accepted it. The
// handle's Cancel withdraws the invocation until it starts. A Schedule never
// carries a scope card, and refuses a method that takes or returns a stream.
func (c *AgentClient) Schedule(at time.Time, method string, args map[string]any) (*golem.Invocation[any], error) {
	m, input, err := c.packNotAwaited("Schedule", method, args)
	if err != nil {
		return nil, err
	}
	ops, err := c.rpc.schedule(at, method, input)
	if err != nil {
		return nil, err
	}
	return c.invocation(m, method, ops), nil
}

// invocation is the handle of an invocation of m, whose result is read
// against the snapshot.
func (c *AgentClient) invocation(m Method, method string, ops link.InvocationOps) *golem.Invocation[any] {
	return link.AnyInvocation(ops, method, func(res witTypes.Option[types.SchemaValueTree]) (any, error) {
		return c.decodeResult(m, method, res)
	}).(*golem.Invocation[any])
}

func optionFromWit[T any](o witTypes.Option[T]) golem.Option[T] {
	if o.IsSome() {
		return golem.Some(o.Some())
	}
	return golem.None[T]()
}

// DiscoverAgentTypes returns a snapshot of every agent type deployed in the
// caller's environment.
func DiscoverAgentTypes() []AgentType { return hostDiscoverAgentTypes() }

// DiscoverAgentType looks one agent type up by name. A missing deployment, or
// one the caller cannot see, reports false rather than failing.
func DiscoverAgentType(name string) (AgentType, bool) { return hostDiscoverAgentType(name) }

// DiscoverAgentTypeByID looks up the deployed agent type that the type tag of
// an agent ID names. Discovery never creates the agent.
func DiscoverAgentTypeByID(id golem.AgentID) (AgentType, bool) {
	return hostDiscoverAgentTypeByID(id)
}

// Upsert returns a client for the keyed agent of the constructor arguments,
// which the host creates when it is missing, exactly as a typed client would.
// For an ephemeral type, each invocation runs on a new one-shot instance.
func (r AgentType) Upsert(ctorArgs map[string]any) (*AgentClient, error) {
	return r.hostUpsert(ctorArgs)
}

// Create creates a durable agent with the constructor arguments and returns a
// client for it once its constructor has finished. Without [Keyed] the new
// agent has a new ID; its options also set configuration overrides
// ([WithConfigJSON], [WithConfigValue]) and environment variables ([WithEnv]).
func (r AgentType) Create(ctorArgs map[string]any, opts ...CreateOpt) (*AgentClient, error) {
	return r.hostCreate(ctorArgs, opts...)
}

// GetByID returns a client for the existing agent of this type with the ID. It
// never creates the agent: an unknown ID, or the ID of an agent of another
// type, is a [golem.RemoteCallError] of kind [golem.RemoteNotFound].
func (r AgentType) GetByID(id golem.AgentID) (*AgentClient, error) {
	return r.hostGetByID(id)
}

// LookupID returns the ID of the keyed agent of the constructor arguments, or
// None when no such agent exists. It never creates the agent.
func (r AgentType) LookupID(ctorArgs map[string]any) (golem.Option[golem.AgentID], error) {
	return r.hostLookupID(ctorArgs)
}

// GetByID returns a dynamic client for the existing agent with the ID, of the
// type its ID names. It never creates the agent.
func GetByID(id golem.AgentID) (*DynamicAgentClient, error) { return hostGetDynamicByID(id) }
