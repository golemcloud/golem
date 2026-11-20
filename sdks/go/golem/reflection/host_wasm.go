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

	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
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

func hostDiscoverAgentTypeByID(id golem.AgentID) (AgentType, bool) {
	found := host.GetAgentTypeByAgentId(agentIDToWit(id))
	if found.IsNone() {
		return AgentType{}, false
	}
	return newAgentType(found.Some().AgentType), true
}

func agentIDToWit(id golem.AgentID) types.AgentId {
	return types.AgentId{Uuid: engine.UUIDToWit(id.UUID())}
}

// witRPC invokes through the host's RPC resource.
type witRPC struct {
	conn   identity.Connection
	target string
}

func (w witRPC) agentID() (golem.AgentID, error) {
	id, failure := identity.AgentID(w.conn)
	if failure != nil {
		return golem.AgentID{}, link.RemoteCallError(w.target, "<agent-id>", failure.RPCError)
	}
	return golem.AgentIDFromUUID(engine.UUIDFromWit(id.Uuid)), nil
}

// start invokes through the asynchronous import, for the same reason
// [golem.MethodDef.Call] does: it is the form a suspended caller is resumed into.
func (w witRPC) start(method string, input types.SchemaValueTree, scopeCard witTypes.Option[*types.PermissionCard]) (pendingRPC, error) {
	inv := w.conn.RPC.AsyncInvokeAndAwait(method, input, scopeCard)
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
	res := w.conn.RPC.Invoke(method, input, noScopeCard())
	if res.IsErr() {
		return golem.InvocationMetadata{}, link.RemoteCallError(w.target, method, res.Err())
	}
	return invocationMetadataFrom(res.Ok()), nil
}

func (w witRPC) schedule(at time.Time, method string, input types.SchemaValueTree) (*golem.ScheduledInvocation, error) {
	res := w.conn.RPC.ScheduleCancelableInvocation(instantFrom(at), method, input, noScopeCard())
	if res.IsErr() {
		return nil, link.RemoteCallError(w.target, method, res.Err())
	}
	receipt := res.Ok()
	return link.ScheduledInvocation(receipt.Metadata, receipt.CancellationToken).(*golem.ScheduledInvocation), nil
}

// newAgentClient is a reflected client of a connection.
func newAgentClient(r AgentType, conn identity.Connection) *AgentClient {
	id := &agentIDCell{}
	if conn.AgentID.IsSome() {
		id = knownAgentID(golem.AgentIDFromUUID(engine.UUIDFromWit(conn.AgentID.Some().Uuid)))
	}
	return &AgentClient{agentType: r, id: id, rpc: witRPC{conn: conn, target: r.Name()}}
}

func (r AgentType) hostUpsert(ctorArgs map[string]any) (*AgentClient, error) {
	ctor, err := r.packConstructor(ctorArgs)
	if err != nil {
		return nil, fmt.Errorf("golem: %s constructor: %w", r.Name(), err)
	}
	conn, failure := identity.Upsert(r.Name(), ctor)
	if failure != nil {
		return nil, link.RemoteCallError(r.Name(), "<upsert>", failure.RPCError)
	}
	return newAgentClient(r, conn), nil
}

func (r AgentType) hostCreate(ctorArgs map[string]any, opts ...CreateOpt) (*AgentClient, error) {
	if r.Mode() == golem.Ephemeral {
		return nil, fmt.Errorf("golem: %s is ephemeral and is not created; address it with Upsert", r.Name())
	}
	ctor, err := r.packConstructor(ctorArgs)
	if err != nil {
		return nil, fmt.Errorf("golem: %s constructor: %w", r.Name(), err)
	}
	options, err := r.createOptions(opts)
	if err != nil {
		return nil, err
	}
	conn, failure := identity.Create(r.Name(), ctor, options)
	if failure != nil {
		return nil, link.RemoteCallError(r.Name(), "<create>", failure.RPCError)
	}
	return newAgentClient(r, conn), nil
}

func (r AgentType) hostGetByID(id golem.AgentID) (*AgentClient, error) {
	if r.Mode() == golem.Ephemeral {
		return nil, fmt.Errorf("golem: %s is ephemeral; an ephemeral agent's ID cannot be addressed", r.Name())
	}
	conn, failure := identity.GetByID(r.Name(), agentIDToWit(id))
	if failure != nil {
		return nil, link.RemoteCallError(id.String(), "<get-by-id>", failure.RPCError)
	}
	return newAgentClient(r, conn), nil
}

func (r AgentType) hostLookupID(ctorArgs map[string]any) (golem.Option[golem.AgentID], error) {
	ctor, err := r.packConstructor(ctorArgs)
	if err != nil {
		return golem.None[golem.AgentID](), fmt.Errorf("golem: %s constructor: %w", r.Name(), err)
	}
	found, failure := identity.Lookup(r.Name(), ctor)
	if failure != nil {
		return golem.None[golem.AgentID](), link.RemoteCallError(r.Name(), "<lookup>", failure.RPCError)
	}
	if found.IsNone() {
		return golem.None[golem.AgentID](), nil
	}
	return golem.Some(golem.AgentIDFromUUID(engine.UUIDFromWit(found.Some().Uuid))), nil
}

func hostGetDynamicByID(id golem.AgentID) (*DynamicAgentClient, error) {
	conn, failure := identity.GetAnyByID(agentIDToWit(id))
	if failure != nil {
		return nil, link.RemoteCallError(id.String(), "<get-by-id>", failure.RPCError)
	}
	return &DynamicAgentClient{agentID: id, typeName: conn.TypeName, rpc: witRPC{conn: conn, target: id.String()}}, nil
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
