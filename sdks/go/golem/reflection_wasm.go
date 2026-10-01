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

//go:build wasip1

package golem

import (
	"fmt"
	"time"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_host"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// DiscoverAgentTypes returns a snapshot of every agent type deployed in the
// caller's environment.
func DiscoverAgentTypes() []ReflectedAgentType {
	registered := host.GetAllAgentTypes()
	out := make([]ReflectedAgentType, 0, len(registered))
	for _, r := range registered {
		out = append(out, newReflectedAgentType(r.AgentType))
	}
	return out
}

// DiscoverAgentType looks one agent type up by name. A missing deployment, or
// one the caller cannot see, reports false rather than failing.
func DiscoverAgentType(name string) (ReflectedAgentType, bool) {
	found := host.GetAgentType(name)
	if found.IsNone() {
		return ReflectedAgentType{}, false
	}
	return newReflectedAgentType(found.Some().AgentType), true
}

// DiscoverAgentTypeByID looks up the agent type behind an existing agent id.
// Discovery never creates the agent.
func DiscoverAgentTypeByID(agentID string) (ReflectedAgentType, bool) {
	found := host.GetAgentTypeByAgentId(agentID)
	if found.IsNone() {
		return ReflectedAgentType{}, false
	}
	return newReflectedAgentType(found.Some().AgentType), true
}

// witRPC invokes through the host's RPC resource.
type witRPC struct {
	rpc    *host.WasmRpc
	target string
}

// call awaits through the asynchronous import, for the same reason
// [MethodDef.Call] does: it is the form a suspended caller is resumed into.
func (w witRPC) call(method string, input types.SchemaValueTree) (witTypes.Option[types.SchemaValueTree], InvocationID, error) {
	inv := w.rpc.AsyncInvokeAndAwait(method, input, noScopeCard())
	id := invocationIDFrom(inv.Metadata)
	res := inv.Future.Get()
	inv.Future.Drop()
	if res.Tag() == witTypes.ResultErr {
		return witTypes.None[types.SchemaValueTree](), id, rpcErrorToGo(w.target, method, res.Err())
	}
	return res.Ok(), id, nil
}

func (w witRPC) trigger(method string, input types.SchemaValueTree) (InvocationID, error) {
	res := w.rpc.Invoke(method, input, noScopeCard())
	if res.IsErr() {
		return InvocationID{}, rpcErrorToGo(w.target, method, res.Err())
	}
	return invocationIDFrom(res.Ok()), nil
}

func (w witRPC) schedule(at time.Time, method string, input types.SchemaValueTree) (*ScheduledInvocation, error) {
	res := w.rpc.ScheduleCancelableInvocation(instantFrom(at), method, input, noScopeCard())
	if res.IsErr() {
		return nil, rpcErrorToGo(w.target, method, res.Err())
	}
	receipt := res.Ok()
	return &ScheduledInvocation{ID: invocationIDFrom(receipt.Metadata), token: receipt.CancellationToken}, nil
}

// Get returns a client for the instance the constructor arguments identify,
// creating it if it does not exist yet, exactly as a typed client would. Pass
// [WithPhantomID] to address a phantom instance.
func (r ReflectedAgentType) Get(ctorArgs map[string]any, opts ...ClientOpt) (*ReflectedAgentClient, error) {
	var o clientOpts
	o.phantomID = witTypes.None[types.Uuid]()
	for _, opt := range opts {
		opt(&o)
	}
	if r.Mode() == Ephemeral && o.phantomID.IsNone() {
		return nil, fmt.Errorf("golem: %s is ephemeral and has no durable identity; use NewPhantom", r.Name())
	}
	ctor, err := r.packConstructor(ctorArgs)
	if err != nil {
		return nil, fmt.Errorf("golem: %s constructor: %w", r.Name(), err)
	}
	agentConfig, cfgErr := reflectedAgentConfig(r, o)
	if cfgErr != nil {
		return nil, cfgErr
	}
	resolved := host.MakeAgentId(r.Name(), ctor, o.phantomID)
	if resolved.IsErr() {
		return nil, fmt.Errorf("golem: %s: %w", r.Name(), agentErrorToGo(resolved.Err()))
	}
	// The fallible form is the reflective one: a caller that built its
	// constructor from a snapshot should get an error, not a trap, when the
	// deployment has moved on.
	created := host.WasmRpcCreate(r.Name(), ctor, o.phantomID, agentConfig)
	if created.Tag() == witTypes.ResultErr {
		return nil, rpcErrorToGo(r.Name(), "<constructor>", created.Err())
	}
	return &ReflectedAgentClient{
		agentType: r,
		agentID:   resolved.Ok(),
		rpc:       witRPC{rpc: created.Ok(), target: r.Name()},
	}, nil
}

// NewPhantom allocates a fresh phantom instance and returns a client for it.
func (r ReflectedAgentType) NewPhantom(ctorArgs map[string]any, opts ...ClientOpt) (*ReflectedAgentClient, error) {
	return r.Get(ctorArgs, append(opts, WithPhantomID(uuidFromWit(apiHost.GenerateIdempotencyKey())))...)
}

// Bind addresses an existing agent by its id, which must name this agent type.
func (r ReflectedAgentType) Bind(agentID string) (*ReflectedAgentClient, error) {
	if r.Mode() == Ephemeral {
		return nil, fmt.Errorf("golem: %s is ephemeral; an ephemeral agent id cannot be bound", r.Name())
	}
	parsed, err := ParseRawAgentID(agentID)
	if err != nil {
		return nil, err
	}
	if parsed.TypeName != r.Name() {
		return nil, fmt.Errorf("golem: agent id %q names %s, not %s", agentID, parsed.TypeName, r.Name())
	}
	rpc, err := bindRPC(parsed)
	if err != nil {
		return nil, err
	}
	return &ReflectedAgentClient{agentType: r, agentID: agentID, rpc: rpc}, nil
}

// AgentID renders the identity the constructor arguments (and phantom, if
// any) name, without creating the agent.
func (r ReflectedAgentType) AgentID(ctorArgs map[string]any, phantom Option[UUID]) (string, error) {
	ctor, err := r.packConstructor(ctorArgs)
	if err != nil {
		return "", fmt.Errorf("golem: %s constructor: %w", r.Name(), err)
	}
	return makeAgentID(r.Name(), ctor, phantom)
}

// reflectedAgentConfig resolves creation-time configuration overrides against
// the snapshot's declarations.
func reflectedAgentConfig(r ReflectedAgentType, o clientOpts) ([]common.TypedAgentConfigValue, error) {
	if len(o.configs) == 0 {
		return nil, nil
	}
	return nil, fmt.Errorf(
		"golem: %s: configuration overrides are not available on reflected clients yet", r.Name())
}

// DiscoverTools returns a snapshot of every tool the calling agent may reach in
// its environment.
func DiscoverTools() []ReflectedTool {
	registered := toolHost.GetAllTools()
	out := make([]ReflectedTool, 0, len(registered))
	for _, r := range registered {
		out = append(out, newReflectedTool(r.LookupName, r.Definition))
	}
	return out
}

// DiscoverTool looks one tool up by its lookup name.
func DiscoverTool(name string) (ReflectedTool, bool) {
	found := toolHost.GetTool(name)
	if found.IsNone() {
		return ReflectedTool{}, false
	}
	return newReflectedTool(found.Some().LookupName, found.Some().Definition), true
}

// ParseRawAgentID takes an agent identity apart without decoding its
// constructor into a Go type. Parsing is strict: a malformed identity is
// reported rather than guessed at.
func ParseRawAgentID(agentID string) (RawAgentID, error) {
	res := host.ParseAgentId(agentID)
	if res.IsErr() {
		return RawAgentID{}, fmt.Errorf("golem: parsing agent id %q: %w", agentID, agentErrorToGo(res.Err()))
	}
	t := res.Ok()
	phantom := None[UUID]()
	if t.F2.IsSome() {
		phantom = Some(uuidFromWit(t.F2.Some()))
	}
	return RawAgentID{TypeName: t.F0, Constructor: TypedValue{wit: t.F1}, PhantomID: phantom}, nil
}

// BindAgentID binds an existing agent identity. It makes no claim about the
// agent's type beyond what the identity says. Binding does not create the
// agent, but the first call to a durable identity does.
func BindAgentID(agentID string) (*DynamicAgentClient, error) {
	parsed, err := ParseRawAgentID(agentID)
	if err != nil {
		return nil, err
	}
	rpc, err := bindRPC(parsed)
	if err != nil {
		return nil, err
	}
	return &DynamicAgentClient{agentID: agentID, parsed: parsed, rpc: rpc}, nil
}

// MakeAgentID renders the identity of an agent of the given type, constructor
// record and phantom, without creating it.
func MakeAgentID(typeName string, constructor TypedValue, phantom Option[UUID]) (string, error) {
	return makeAgentID(typeName, constructor.wit.Value, phantom)
}

func makeAgentID(typeName string, ctor types.SchemaValueTree, phantom Option[UUID]) (string, error) {
	p := witTypes.None[types.Uuid]()
	if u, has := phantom.Get(); has {
		p = witTypes.Some(uuidToWit(u))
	}
	res := host.MakeAgentId(typeName, ctor, p)
	if res.IsErr() {
		return "", fmt.Errorf("golem: %s: %w", typeName, agentErrorToGo(res.Err()))
	}
	return res.Ok(), nil
}

func bindRPC(parsed RawAgentID) (witRPC, error) {
	phantom := witTypes.None[types.Uuid]()
	if id, present := parsed.PhantomID.Get(); present {
		phantom = witTypes.Some(uuidToWit(id))
	}
	created := host.WasmRpcCreate(parsed.TypeName, parsed.Constructor.wit.Value, phantom, nil)
	if created.Tag() == witTypes.ResultErr {
		return witRPC{}, rpcErrorToGo(parsed.TypeName, "<bind>", created.Err())
	}
	return witRPC{rpc: created.Ok(), target: parsed.TypeName}, nil
}
