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
	"reflect"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// A Client addresses one agent instance for remote calls. It is produced by
// [ClientFor] and consumed by the call methods on [MethodDef].
//
// The Id type parameter is what makes cross-agent calls type-safe: a
// MethodDef[PaymentId, …] only accepts a Client[PaymentId], so aiming a method
// at the wrong agent is a compile error rather than a runtime not-found.
//
// Client carries no type parameter for state: the caller does not know, and
// must not depend on, how the target stores anything.
type Client[Id any] struct {
	rpc       *host.WasmRpc
	agentType string
	agentID   string
}

// AgentID returns the target's agent id, as resolved by the host.
func (c Client[Id]) AgentID() string { return c.agentID }

// ClientOpt configures a client.
type ClientOpt func(*clientOpts)

type clientOpts struct {
	phantomID witTypes.Option[types.Uuid]
	configs   []configOverrideFn
}

// WithPhantomID addresses a specific phantom instance of the target agent.
// Use [NewPhantom] to allocate a fresh one.
func WithPhantomID(id types.Uuid) ClientOpt {
	return func(o *clientOpts) { o.phantomID = witTypes.Some(id) }
}

// ClientFor returns a client addressing the agent instance identified by id.
//
// The id is encoded with the same codecs the target uses to decode its
// constructor parameters — they are derived from the same Go types — so caller
// and callee agree by construction rather than by convention.
func ClientFor[Id any, S any](a *Agent[Id, S], id Id, opts ...ClientOpt) (Client[Id], error) {
	e := defs.agents[a.name]
	if e == nil {
		return Client[Id]{}, fmt.Errorf("golem: ClientFor: unknown agent %s", a.name)
	}

	var o clientOpts
	o.phantomID = witTypes.None[types.Uuid]()
	for _, f := range opts {
		f(&o)
	}

	idVal := reflect.ValueOf(&id).Elem()
	ctor := encodeParams(e.idFields, idVal)

	// Encode and validate any config overrides against the target's declarations
	// before touching the host, so a mistyped or undeclared key is a clear error.
	agentConfig, err := buildAgentConfig(defs, e, o.configs)
	if err != nil {
		return Client[Id]{}, fmt.Errorf("golem: ClientFor %s: %w", a.name, err)
	}

	// Resolve the id up front: it is wanted for error messages, and a failure
	// here means the constructor parameters are wrong — better to surface that
	// now than as an opaque not-found on the first call.
	resolved := host.MakeAgentId(a.name, ctor, o.phantomID)
	if resolved.IsErr() {
		return Client[Id]{}, fmt.Errorf("golem: ClientFor %s: %w", a.name, agentErrorToGo(resolved.Err()))
	}

	return Client[Id]{
		rpc:       host.MakeWasmRpc(a.name, ctor, o.phantomID, agentConfig),
		agentType: a.name,
		agentID:   resolved.Ok(),
	}, nil
}

// NewPhantom allocates a fresh phantom instance of the target agent and returns
// a client for it, together with the phantom id needed to address it again.
//
// Ephemeral agents have no durable identity, so this is the only way to obtain
// a client for one.
func NewPhantom[Id any, S any](a *Agent[Id, S], id Id) (Client[Id], types.Uuid, error) {
	phantom := apiHost.GenerateIdempotencyKey()
	c, err := ClientFor(a, id, WithPhantomID(phantom))
	return c, phantom, err
}

// agentErrorToGo converts a host agent-error into a Go error, keeping the case
// distinguishable rather than flattening everything to a string.
func agentErrorToGo(e common.AgentError) error {
	switch e.Tag() {
	case common.AgentErrorInvalidInput:
		return fmt.Errorf("invalid input: %s", e.InvalidInput())
	case common.AgentErrorInvalidMethod:
		return fmt.Errorf("invalid method: %s", e.InvalidMethod())
	case common.AgentErrorInvalidType:
		return fmt.Errorf("invalid type: %s", e.InvalidType())
	case common.AgentErrorCustomError:
		return fmt.Errorf("custom error: %v", e.CustomError())
	default:
		return fmt.Errorf("agent error (tag %d)", e.Tag())
	}
}
