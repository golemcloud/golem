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
//	result, id, err := client.Call("greet", map[string]any{"greeting": "hi"})
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
		output := golem.None[core.Ref]()
		if m.OutputSchema.Tag() == common.OutputSchemaSingle {
			ref, err := conv.Ref(m.OutputSchema.Single())
			if err != nil {
				panic(fmt.Errorf("golem: agent type %q method %q result: %w", wit.TypeName, m.Name, err))
			}
			output = golem.Some(ref)
		}
		methods = append(methods, Method{wit: m, input: input, output: output})
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
	wit    common.AgentMethod
	input  core.Ref
	output golem.Option[core.Ref]
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
	agentID   string
	rpc       agentRPC
}

// agentRPC is the host connection a reflected or dynamic agent client invokes
// through. The interface keeps packing, validation and decoding testable
// without a host.
type agentRPC interface {
	start(method string, input types.SchemaValueTree) (pendingRPC, error)
	trigger(method string, input types.SchemaValueTree) (golem.InvocationID, error)
	schedule(at time.Time, method string, input types.SchemaValueTree) (*golem.ScheduledInvocation, error)
}

// pendingRPC is an invocation in flight: its identity, its outcome, and a way
// to cancel it.
type pendingRPC struct {
	id     golem.InvocationID
	wait   func() (witTypes.Option[types.SchemaValueTree], error)
	cancel func()
}

// ErrCallCancelled is what waiting on a call cancelled with
// [PendingCall.Cancel] reports. Cancelling only stops waiting: the call may
// already have run.
var ErrCallCancelled = errors.New("golem: the call was cancelled before its result was awaited")

// PendingCall is a reflected or dynamic invocation in flight.
type PendingCall[T any] struct {
	// ID identifies the invocation, available before the result is.
	ID     golem.InvocationID
	wait   func() (T, error)
	cancel func()
	done   bool
	out    T
	err    error
}

// Wait waits for the invocation to finish and returns its result.
func (p *PendingCall[T]) Wait() (T, error) {
	if !p.done {
		p.done = true
		p.out, p.err = p.wait()
	}
	return p.out, p.err
}

// Cancel makes a best-effort attempt to cancel the invocation; a later Wait
// reports [ErrCallCancelled]. It does nothing once the result was awaited.
func (p *PendingCall[T]) Cancel() {
	if p.done {
		return
	}
	p.done = true
	p.cancel()
	p.err = ErrCallCancelled
}

// AgentType returns the snapshot this client was built from.
func (c *AgentClient) AgentType() AgentType { return c.agentType }

// AgentID returns the target's agent id, as resolved by the host.
func (c *AgentClient) AgentID() string { return c.agentID }

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

// Call invokes a method with named arguments and waits for its result, as
// canonical JSON (nil when the method returns nothing), together with the
// invocation's identity.
func (c *AgentClient) Call(method string, args map[string]any) (any, golem.InvocationID, error) {
	p, err := c.CallAsync(method, args)
	if err != nil {
		return nil, golem.InvocationID{}, err
	}
	out, err := p.Wait()
	return out, p.ID, err
}

// CallAsync invokes a method with named arguments and returns at once; the
// result is read, as for Call, with the pending call's Wait.
func (c *AgentClient) CallAsync(method string, args map[string]any) (*PendingCall[any], error) {
	m, input, err := c.pack(method, args)
	if err != nil {
		return nil, err
	}
	p, err := c.rpc.start(method, input)
	if err != nil {
		return nil, err
	}
	return &PendingCall[any]{ID: p.id, cancel: p.cancel, wait: func() (any, error) {
		res, err := p.wait()
		if err != nil {
			return nil, err
		}
		return c.decodeResult(m, method, res)
	}}, nil
}

// decodeResult reads a method's result against the snapshot.
func (c *AgentClient) decodeResult(m Method, method string, res witTypes.Option[types.SchemaValueTree]) (any, error) {
	out, declared := m.Output().Get()
	tree, has := optionFromWit(res).Get()
	switch {
	case has && !declared:
		// Cardinality is part of the contract, so an unexpected value is a
		// remote output error rather than something to quietly drop.
		return nil, fmt.Errorf("golem: %s.%s returned a value but declares none", c.agentType.Name(), method)
	case !has && declared:
		return nil, fmt.Errorf("golem: %s.%s returned nothing but declares a result", c.agentType.Name(), method)
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
	return nil, fmt.Errorf("golem: %s.%s returned an unreadable result: %w", c.agentType.Name(), method, err)
}

// Trigger invokes a method without waiting for its result.
func (c *AgentClient) Trigger(method string, args map[string]any) (golem.InvocationID, error) {
	_, input, err := c.pack(method, args)
	if err != nil {
		return golem.InvocationID{}, err
	}
	return c.rpc.trigger(method, input)
}

// Schedule arranges for a method to be invoked at the given time.
func (c *AgentClient) Schedule(at time.Time, method string, args map[string]any) (*golem.ScheduledInvocation, error) {
	_, input, err := c.pack(method, args)
	if err != nil {
		return nil, err
	}
	return c.rpc.schedule(at, method, input)
}

func optionFromWit[T any](o witTypes.Option[T]) golem.Option[T] {
	if o.IsSome() {
		return golem.Some(o.Some())
	}
	return golem.None[T]()
}
