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
	"time"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"github.com/golemcloud/golem/sdks/go/golem/internal/identity"
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
	clock "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_clocks_0_3_0_system_clock"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

func hostDiscoverAgentTypes() []AgentType {
	registered := host.GetAllAgentTypes()
	out := make([]AgentType, 0, len(registered))
	for _, r := range registered {
		out = append(out, newAgentType(r.AgentType))
	}
	return out
}

func hostDiscoverAgentType(name string) (AgentType, bool) {
	found := host.GetAgentType(name)
	if found.IsNone() {
		return AgentType{}, false
	}
	return newAgentType(found.Some().AgentType), true
}

func hostDiscoverAgentTypeByID(agentID string) (AgentType, bool) {
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
func (w witRPC) start(method string, input types.SchemaValueTree, scopeCard witTypes.Option[*types.PermissionCard]) (pendingRPC, error) {
	inv := w.rpc.AsyncInvokeAndAwait(method, input, scopeCard)
	return pendingRPC{
		metadata: asyncInvocationMetadata(inv.Metadata),
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

func (w witRPC) trigger(method string, input types.SchemaValueTree) (golem.InvocationMetadata, error) {
	res := w.rpc.Invoke(method, input, noScopeCard())
	if res.IsErr() {
		return golem.InvocationMetadata{}, link.RemoteCallError(w.target, method, res.Err())
	}
	return invocationMetadataFrom(res.Ok()), nil
}

func (w witRPC) schedule(at time.Time, method string, input types.SchemaValueTree) (*golem.ScheduledInvocation, error) {
	res := w.rpc.ScheduleCancelableInvocation(instantFrom(at), method, input, noScopeCard())
	if res.IsErr() {
		return nil, link.RemoteCallError(w.target, method, res.Err())
	}
	receipt := res.Ok()
	return link.ScheduledInvocation(receipt.Metadata, receipt.CancellationToken).(*golem.ScheduledInvocation), nil
}

func (r AgentType) hostGet(ctorArgs map[string]any, opts ...ClientOpt) (*AgentClient, error) {
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
	// The fallible form is the reflective one: a caller that built its
	// constructor from a snapshot should get an error, not a trap, when the
	// deployment has moved on.
	connected, failure := identity.Construct(r.Name(), ctor, o.phantomID, agentConfig)
	if failure != nil {
		if failure.Stage == identity.ConstructStage {
			return nil, fmt.Errorf("golem: %s: %w", r.Name(), link.AgentError(failure.AgentError))
		}
		return nil, link.RemoteCallError(r.Name(), "<constructor>", failure.RPCError)
	}
	return &AgentClient{
		agentType: r,
		agentID:   connected.AgentID,
		rpc:       witRPC{rpc: connected.RPC, target: r.Name()},
	}, nil
}

func (r AgentType) hostNewPhantom(ctorArgs map[string]any, opts ...ClientOpt) (*AgentClient, error) {
	return r.Get(ctorArgs, append(opts, WithPhantomID(engine.UUIDFromWit(apiHost.GenerateIdempotencyKey())))...)
}

func (r AgentType) hostBind(agentID string, opts ...ClientOpt) (*AgentClient, error) {
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
	rpc, err := bindRPC(agentID, parsed, agentConfig)
	if err != nil {
		return nil, err
	}
	return &AgentClient{agentType: r, agentID: agentID, rpc: rpc}, nil
}

func (r AgentType) hostAgentID(ctorArgs map[string]any, phantom golem.Option[golem.UUID]) (string, error) {
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

func hostParseRawAgentID(agentID string) (RawAgentID, error) {
	parsed, failure := identity.Parse(agentID)
	if failure != nil {
		return RawAgentID{}, fmt.Errorf("golem: parsing agent id %q: %w", agentID, link.AgentError(failure.AgentError))
	}
	phantom := golem.None[golem.UUID]()
	if parsed.PhantomID.IsSome() {
		phantom = golem.Some(engine.UUIDFromWit(parsed.PhantomID.Some()))
	}
	return RawAgentID{TypeName: parsed.TypeName, Constructor: link.TypedValue(parsed.Constructor).(golem.TypedValue), PhantomID: phantom}, nil
}

func hostBindAgentID(agentID string) (*DynamicAgentClient, error) {
	parsed, err := ParseRawAgentID(agentID)
	if err != nil {
		return nil, err
	}
	rpc, err := bindRPC(agentID, parsed, nil)
	if err != nil {
		return nil, err
	}
	return &DynamicAgentClient{agentID: agentID, parsed: parsed, rpc: rpc}, nil
}

func hostMakeAgentID(typeName string, constructor golem.TypedValue, phantom golem.Option[golem.UUID]) (string, error) {
	return makeAgentID(typeName, link.TypedValueWit(constructor).Value, phantom)
}

func makeAgentID(typeName string, ctor types.SchemaValueTree, phantom golem.Option[golem.UUID]) (string, error) {
	p := witTypes.None[types.Uuid]()
	if u, has := phantom.Get(); has {
		p = witTypes.Some(engine.UUIDToWit(u))
	}
	resolved, failure := identity.Render(typeName, ctor, p)
	if failure != nil {
		return "", fmt.Errorf("golem: %s: %w", typeName, link.AgentError(failure.AgentError))
	}
	return resolved, nil
}

func bindRPC(agentID string, parsed RawAgentID, agentConfig []common.TypedAgentConfigValue) (witRPC, error) {
	phantom := witTypes.None[types.Uuid]()
	if id, present := parsed.PhantomID.Get(); present {
		phantom = witTypes.Some(engine.UUIDToWit(id))
	}
	connected, failure := identity.Bind(agentID, identity.Parsed{
		TypeName:    parsed.TypeName,
		Constructor: link.TypedValueWit(parsed.Constructor),
		PhantomID:   phantom,
	}, agentConfig)
	if failure != nil {
		return witRPC{}, link.RemoteCallError(parsed.TypeName, "<bind>", failure.RPCError)
	}
	return witRPC{rpc: connected.RPC, target: parsed.TypeName}, nil
}

// noScopeCard is the scope card of the invocations that do not wait for their
// result: the host accepts a scope card only on an awaited call.
func noScopeCard() witTypes.Option[*types.PermissionCard] {
	return witTypes.None[*types.PermissionCard]()
}

func invocationMetadataFrom(m host.InvocationMetadata) golem.InvocationMetadata {
	return link.InvocationMetadata(m).(golem.InvocationMetadata)
}

// asyncInvocationMetadata is the identity of an asynchronous invocation, or nil
// when the host rejected the call before it started; its future reports the
// rejection.
func asyncInvocationMetadata(m witTypes.Option[host.InvocationMetadata]) *golem.InvocationMetadata {
	if m.IsNone() {
		return nil
	}
	metadata := invocationMetadataFrom(m.Some())
	return &metadata
}

func instantFrom(t time.Time) clock.Instant {
	return clock.Instant{Seconds: t.Unix(), Nanoseconds: uint32(t.Nanosecond())}
}
