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

package reflection

import (
	"fmt"
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
	clock "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_clocks_0_3_0_system_clock"
	"time"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// DiscoverAgentTypes returns a snapshot of every agent type deployed in the
// caller's environment.
func DiscoverAgentTypes() []AgentType {
	registered := host.GetAllAgentTypes()
	out := make([]AgentType, 0, len(registered))
	for _, r := range registered {
		out = append(out, newAgentType(r.AgentType))
	}
	return out
}

// DiscoverAgentType looks one agent type up by name. A missing deployment, or
// one the caller cannot see, reports false rather than failing.
func DiscoverAgentType(name string) (AgentType, bool) {
	found := host.GetAgentType(name)
	if found.IsNone() {
		return AgentType{}, false
	}
	return newAgentType(found.Some().AgentType), true
}

// DiscoverAgentTypeByID looks up the agent type behind an existing agent id.
// Discovery never creates the agent.
func DiscoverAgentTypeByID(agentID string) (AgentType, bool) {
	found := host.GetAgentTypeByAgentId(agentID)
	if found.IsNone() {
		return AgentType{}, false
	}
	return newAgentType(found.Some().AgentType), true
}

// witRPC invokes through the host's RPC resource.
type witRPC struct {
	rpc    *host.WasmRpc
	target string
}

// start invokes through the asynchronous import, for the same reason
// [golem.MethodDef.Call] does: it is the form a suspended caller is resumed into.
func (w witRPC) start(method string, input types.SchemaValueTree) (pendingRPC, error) {
	inv := w.rpc.AsyncInvokeAndAwait(method, input, noScopeCard())
	return pendingRPC{
		id: invocationIDFrom(inv.Metadata),
		wait: func() (witTypes.Option[types.SchemaValueTree], error) {
			res := inv.Future.Get()
			inv.Future.Drop()
			if res.Tag() == witTypes.ResultErr {
				return witTypes.None[types.SchemaValueTree](), link.RemoteCallError(w.target, method, res.Err())
			}
			return res.Ok(), nil
		},
		cancel: func() {
			inv.Future.Cancel()
			inv.Future.Drop()
		},
	}, nil
}

func (w witRPC) trigger(method string, input types.SchemaValueTree) (golem.InvocationID, error) {
	res := w.rpc.Invoke(method, input, noScopeCard())
	if res.IsErr() {
		return golem.InvocationID{}, link.RemoteCallError(w.target, method, res.Err())
	}
	return invocationIDFrom(res.Ok()), nil
}

func (w witRPC) schedule(at time.Time, method string, input types.SchemaValueTree) (*golem.ScheduledInvocation, error) {
	res := w.rpc.ScheduleCancelableInvocation(instantFrom(at), method, input, noScopeCard())
	if res.IsErr() {
		return nil, link.RemoteCallError(w.target, method, res.Err())
	}
	receipt := res.Ok()
	return link.ScheduledInvocation(receipt.Metadata.AgentId, receipt.Metadata.IdempotencyKey, receipt.CancellationToken).(*golem.ScheduledInvocation), nil
}

// Get returns a client for the instance the constructor arguments identify,
// creating it if it does not exist yet, exactly as a typed client would. Pass
// [WithPhantomID] to address a phantom instance.
func (r AgentType) Get(ctorArgs map[string]any, opts ...ClientOpt) (*AgentClient, error) {
	o := applyOpts(opts)
	if r.Mode() == golem.Ephemeral && o.phantomID.IsNone() {
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
		return nil, fmt.Errorf("golem: %s: %w", r.Name(), link.AgentError(resolved.Err()))
	}
	// The fallible form is the reflective one: a caller that built its
	// constructor from a snapshot should get an error, not a trap, when the
	// deployment has moved on.
	created := host.WasmRpcCreate(r.Name(), ctor, o.phantomID, agentConfig)
	if created.Tag() == witTypes.ResultErr {
		return nil, link.RemoteCallError(r.Name(), "<constructor>", created.Err())
	}
	return &AgentClient{
		agentType: r,
		agentID:   resolved.Ok(),
		rpc:       witRPC{rpc: created.Ok(), target: r.Name()},
	}, nil
}

// NewPhantom allocates a fresh phantom instance and returns a client for it.
func (r AgentType) NewPhantom(ctorArgs map[string]any, opts ...ClientOpt) (*AgentClient, error) {
	return r.Get(ctorArgs, append(opts, WithPhantomID(engine.UUIDFromWit(apiHost.GenerateIdempotencyKey())))...)
}

// Bind addresses an agent by its id, which must name this agent type. Binding
// does not create it; the first call to a durable id does, with the
// configuration overrides given here. An agent that already exists keeps the
// configuration it was created with.
func (r AgentType) Bind(agentID string, opts ...ClientOpt) (*AgentClient, error) {
	if r.Mode() == golem.Ephemeral {
		return nil, fmt.Errorf("golem: %s is ephemeral; an ephemeral agent id cannot be bound", r.Name())
	}
	parsed, err := ParseRawAgentID(agentID)
	if err != nil {
		return nil, err
	}
	if parsed.TypeName != r.Name() {
		return nil, fmt.Errorf("golem: agent id %q names %s, not %s", agentID, parsed.TypeName, r.Name())
	}
	o := applyOpts(opts)
	if o.phantomID.IsSome() {
		return nil, fmt.Errorf("golem: %s: an agent id already names its phantom; WithPhantomID does not apply to Bind", r.Name())
	}
	agentConfig, err := reflectedAgentConfig(r, o)
	if err != nil {
		return nil, err
	}
	rpc, err := bindRPC(parsed, agentConfig)
	if err != nil {
		return nil, err
	}
	return &AgentClient{agentType: r, agentID: agentID, rpc: rpc}, nil
}

// AgentID renders the identity the constructor arguments (and phantom, if
// any) name, without creating the agent.
func (r AgentType) AgentID(ctorArgs map[string]any, phantom golem.Option[golem.UUID]) (string, error) {
	ctor, err := r.packConstructor(ctorArgs)
	if err != nil {
		return "", fmt.Errorf("golem: %s constructor: %w", r.Name(), err)
	}
	return makeAgentID(r.Name(), ctor, phantom)
}

// reflectedAgentConfig resolves creation-time configuration overrides against
// the snapshot's declarations.
func reflectedAgentConfig(r AgentType, o clientOpts) ([]common.TypedAgentConfigValue, error) {
	return r.configValues(o.overrides)
}

// ParseRawAgentID takes an agent identity apart without decoding its
// constructor into a Go type. Parsing is strict: a malformed identity is
// reported rather than guessed at.
func ParseRawAgentID(agentID string) (RawAgentID, error) {
	res := host.ParseAgentId(agentID)
	if res.IsErr() {
		return RawAgentID{}, fmt.Errorf("golem: parsing agent id %q: %w", agentID, link.AgentError(res.Err()))
	}
	t := res.Ok()
	phantom := golem.None[golem.UUID]()
	if t.F2.IsSome() {
		phantom = golem.Some(engine.UUIDFromWit(t.F2.Some()))
	}
	return RawAgentID{TypeName: t.F0, Constructor: link.TypedValue(t.F1).(golem.TypedValue), PhantomID: phantom}, nil
}

// BindAgentID binds an existing agent identity. It makes no claim about the
// agent's type beyond what the identity says. Binding does not create the
// agent, but the first call to a durable identity does.
func BindAgentID(agentID string) (*DynamicAgentClient, error) {
	parsed, err := ParseRawAgentID(agentID)
	if err != nil {
		return nil, err
	}
	rpc, err := bindRPC(parsed, nil)
	if err != nil {
		return nil, err
	}
	return &DynamicAgentClient{agentID: agentID, parsed: parsed, rpc: rpc}, nil
}

// MakeAgentID renders the identity of an agent of the given type, constructor
// record and phantom, without creating it.
func MakeAgentID(typeName string, constructor golem.TypedValue, phantom golem.Option[golem.UUID]) (string, error) {
	return makeAgentID(typeName, link.TypedValueWit(constructor).Value, phantom)
}

func makeAgentID(typeName string, ctor types.SchemaValueTree, phantom golem.Option[golem.UUID]) (string, error) {
	p := witTypes.None[types.Uuid]()
	if u, has := phantom.Get(); has {
		p = witTypes.Some(engine.UUIDToWit(u))
	}
	res := host.MakeAgentId(typeName, ctor, p)
	if res.IsErr() {
		return "", fmt.Errorf("golem: %s: %w", typeName, link.AgentError(res.Err()))
	}
	return res.Ok(), nil
}

func bindRPC(parsed RawAgentID, agentConfig []common.TypedAgentConfigValue) (witRPC, error) {
	phantom := witTypes.None[types.Uuid]()
	if id, present := parsed.PhantomID.Get(); present {
		phantom = witTypes.Some(engine.UUIDToWit(id))
	}
	created := host.WasmRpcCreate(parsed.TypeName, link.TypedValueWit(parsed.Constructor).Value, phantom, agentConfig)
	if created.Tag() == witTypes.ResultErr {
		return witRPC{}, link.RemoteCallError(parsed.TypeName, "<bind>", created.Err())
	}
	return witRPC{rpc: created.Ok(), target: parsed.TypeName}, nil
}

// noScopeCard is the permission scope card sent with every outgoing
// invocation: none, as for the typed clients.
func noScopeCard() witTypes.Option[*types.PermissionCard] {
	return witTypes.None[*types.PermissionCard]()
}

func invocationIDFrom(m host.InvocationMetadata) golem.InvocationID {
	return golem.InvocationID{AgentID: m.AgentId, IdempotencyKey: m.IdempotencyKey}
}

func instantFrom(t time.Time) clock.Instant {
	return clock.Instant{Seconds: t.Unix(), Nanoseconds: uint32(t.Nanosecond())}
}
