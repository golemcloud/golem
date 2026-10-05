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
	"slices"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Client addresses one agent instance for remote calls. It is produced by
// [AgentDefinition.Get] and consumed by the call methods on [MethodDef].
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
	phantomID Option[UUID]
}

// AgentID returns the target's agent id, as resolved by the host.
func (c Client[Id]) AgentID() string { return c.agentID }

// PhantomID returns the phantom instance id this client addresses, or None for a
// durable client. A [AgentDefinition.NewPhantom] client carries the freshly allocated id here
// (re-address it later with [WithPhantomID]).
func (c Client[Id]) PhantomID() Option[UUID] { return c.phantomID }

// ClientOpt configures a client.
type ClientOpt func(*clientOpts)

type clientOpts struct {
	phantomID witTypes.Option[types.Uuid]
	configs   []configOverrideFn
	// overrides are untyped configuration entries, for reflected clients.
	overrides []configOverride
}

// WithPhantomID addresses a specific phantom instance of the target agent.
// Use [AgentDefinition.NewPhantom] to allocate a fresh one.
func WithPhantomID(id UUID) ClientOpt {
	return func(o *clientOpts) { o.phantomID = witTypes.Some(uuidToWit(id)) }
}

// Get returns a client addressing the agent instance identified by id, or panics
// if the target can't be resolved (unknown agent, a bad config override, or
// invalid constructor parameters). Such a failure is a programming or
// configuration error with no in-band recovery, so it panics rather than
// returning an error; the panic surfaces to the caller as an agent-error.
//
// The id is encoded with the same codecs the target uses to decode its
// constructor parameters — they are derived from the same Go types — so caller
// and callee agree by construction rather than by convention.
func (a *AgentDefinition[Id, Cfg]) Get(id Id, opts ...ClientOpt) Client[Id] {
	return getClient[Id](defs, a.name, id, opts)
}

// getClient builds a client for an agent registered under name. It is shared by
// the local ([AgentDefinition.Get]) and client ([FullAgentClient.Get]) paths,
// which differ only in how the target was declared, never in how it is called.
func getClient[Id any](d *definitions, name string, id Id, opts []ClientOpt) Client[Id] {
	e := d.agents[name]
	if e == nil {
		panic(fmt.Errorf("golem: Get: unknown agent %s", name))
	}

	var o clientOpts
	o.phantomID = witTypes.None[types.Uuid]()
	for _, f := range opts {
		f(&o)
	}
	if err := requireIdentity(e, o.phantomID.IsSome()); err != nil {
		panic(err)
	}
	if len(o.overrides) > 0 {
		panic(fmt.Errorf("golem: Get %s: WithConfigJSON and WithConfigValue configure reflected clients; use WithConfig", name))
	}

	idVal := reflect.ValueOf(&id).Elem()
	ctor := encodeParams(e.idFields, idVal)

	// Encode and validate any config overrides against the target's declarations
	// before touching the host, so a mistyped or undeclared key is a clear error.
	agentConfig, err := buildAgentConfig(d, e, o.configs)
	if err != nil {
		panic(fmt.Errorf("golem: Get %s: %w", name, err))
	}

	// Resolve the id up front: it is wanted for error messages, and a failure
	// here means the constructor parameters are wrong — better to surface that
	// now than as an opaque not-found on the first call.
	resolved := host.MakeAgentId(name, ctor, o.phantomID)
	if resolved.IsErr() {
		panic(fmt.Errorf("golem: Get %s: %w", name, agentErrorToGo(resolved.Err())))
	}

	phantomID := None[UUID]()
	if o.phantomID.IsSome() {
		phantomID = Some(uuidFromWit(o.phantomID.Some()))
	}
	return Client[Id]{
		rpc:       host.MakeWasmRpc(name, ctor, o.phantomID, agentConfig),
		agentType: name,
		agentID:   resolved.Ok(),
		phantomID: phantomID,
	}
}

// requireIdentity refuses to address an ephemeral agent without a phantom: it
// has no durable identity to get.
func requireIdentity(e *agentEntry, phantom bool) error {
	if e.mode == common.AgentModeEphemeral && !phantom {
		return fmt.Errorf("golem: Get %s: an ephemeral agent has no durable identity; use NewPhantom", e.name)
	}
	return nil
}

// newPhantomClient allocates a fresh phantom instance and returns a client for
// it, shared by the local and remote paths.
func newPhantomClient[Id any](d *definitions, name string, id Id, opts []ClientOpt) Client[Id] {
	phantom := uuidFromWit(apiHost.GenerateIdempotencyKey())
	return getClient[Id](d, name, id, append(slices.Clone(opts), WithPhantomID(phantom)))
}

// Bind addresses an existing agent by its id, which must name this agent type
// and carry a constructor of shape Id. Binding does not create the agent, but
// the first call to a durable identity does. An ephemeral agent's id cannot be
// bound: it named one invocation.
func (a *AgentDefinition[Id, Cfg]) Bind(agentID string) (Client[Id], error) {
	return bindTypedClient[Id](defs, a.name, agentID)
}

// AgentID renders the identity of the instance id (and phantom, if any)
// names, without creating it.
func (a *AgentDefinition[Id, Cfg]) AgentID(id Id, phantom Option[UUID]) (string, error) {
	return makeTypedAgentID[Id](defs, a.name, id, phantom)
}

// bindTypedClient binds an agent id to a definition: the id must name the
// type, its constructor must decode into Id, and the type must be durable.
func bindTypedClient[Id any](d *definitions, name, agentID string) (Client[Id], error) {
	parsed, err := ParseAgentID[Id](agentID)
	if err != nil {
		return Client[Id]{}, err
	}
	if parsed.TypeName != name {
		return Client[Id]{}, fmt.Errorf("golem: agent id %q names %s, not %s", agentID, parsed.TypeName, name)
	}
	if e := d.agents[name]; e != nil && e.mode == common.AgentModeEphemeral {
		return Client[Id]{}, fmt.Errorf("golem: %s is ephemeral; an ephemeral agent id cannot be bound", name)
	}
	return bindClient[Id](agentID)
}

// bindClient addresses the agent agentID names. The host parses the id;
// nothing about the target is checked here.
func bindClient[Id any](agentID string) (Client[Id], error) {
	res := host.ParseAgentId(agentID)
	if res.IsErr() {
		return Client[Id]{}, fmt.Errorf("golem: parsing agent id %q: %w", agentID, agentErrorToGo(res.Err()))
	}
	t := res.Ok()
	phantomID := None[UUID]()
	if t.F2.IsSome() {
		phantomID = Some(uuidFromWit(t.F2.Some()))
	}
	return Client[Id]{
		rpc:       host.MakeWasmRpc(t.F0, t.F1.Value, t.F2, nil),
		agentType: t.F0,
		agentID:   agentID,
		phantomID: phantomID,
	}, nil
}

// makeTypedAgentID renders an agent id from a typed constructor value.
func makeTypedAgentID[Id any](d *definitions, name string, id Id, phantom Option[UUID]) (string, error) {
	e := d.agents[name]
	if e == nil {
		return "", fmt.Errorf("golem: unknown agent %s", name)
	}
	ctor := encodeParams(e.idFields, reflect.ValueOf(&id).Elem())
	p := witTypes.None[types.Uuid]()
	if u, has := phantom.Get(); has {
		p = witTypes.Some(uuidToWit(u))
	}
	res := host.MakeAgentId(name, ctor, p)
	if res.IsErr() {
		return "", fmt.Errorf("golem: %s: %w", name, agentErrorToGo(res.Err()))
	}
	return res.Ok(), nil
}

// NewPhantom allocates a fresh phantom instance of the target agent and returns
// a client for it (panicking on failure, like [AgentDefinition.Get]). The freshly
// allocated phantom id rides on the client — read it back with [Client.PhantomID]
// to address the same instance again (via [WithPhantomID]).
//
// Ephemeral agents have no durable identity, so this is the only way to obtain
// a client for one.
func (a *AgentDefinition[Id, Cfg]) NewPhantom(id Id, opts ...ClientOpt) Client[Id] {
	return newPhantomClient[Id](defs, a.name, id, opts)
}

// AgentErrorKind classifies an [AgentError].
type AgentErrorKind uint8

const (
	// AgentInvalidInput means the input did not match the method's parameters.
	AgentInvalidInput AgentErrorKind = iota
	// AgentInvalidMethod means no such method exists on the agent.
	AgentInvalidMethod
	// AgentInvalidType means no such agent type exists.
	AgentInvalidType
	// AgentCustom means the agent failed (a returned error or a panic).
	AgentCustom
	// AgentUnknown is an error kind the SDK does not recognize.
	AgentUnknown
)

// AgentError is a typed error from invoking an agent. Its Kind lets callers
// classify the failure with errors.As, rather than string-matching the message.
type AgentError struct {
	Kind    AgentErrorKind
	Message string
}

func (e *AgentError) Error() string {
	switch e.Kind {
	case AgentInvalidInput:
		return "invalid input: " + e.Message
	case AgentInvalidMethod:
		return "invalid method: " + e.Message
	case AgentInvalidType:
		return "invalid type: " + e.Message
	case AgentCustom:
		return "custom error: " + e.Message
	default:
		return e.Message
	}
}

// agentErrorToGo converts a host agent-error into a typed [AgentError], keeping
// the case inspectable rather than flattening everything to a string.
func agentErrorToGo(e common.AgentError) error {
	switch e.Tag() {
	case common.AgentErrorInvalidInput:
		return &AgentError{Kind: AgentInvalidInput, Message: e.InvalidInput()}
	case common.AgentErrorInvalidMethod:
		return &AgentError{Kind: AgentInvalidMethod, Message: e.InvalidMethod()}
	case common.AgentErrorInvalidType:
		return &AgentError{Kind: AgentInvalidType, Message: e.InvalidType()}
	case common.AgentErrorCustomError:
		return &AgentError{Kind: AgentCustom, Message: customErrorMessage(e.CustomError())}
	default:
		return &AgentError{Kind: AgentUnknown, Message: fmt.Sprintf("agent error (tag %d)", e.Tag())}
	}
}

// customErrorMessage recovers the string payload the SDK encodes into the
// custom-error case (see customError). Without this the TypedSchemaValue would
// render as a struct dump, and — on the RPC path — a remote agent's panic message
// would reach the caller as that dump. Non-string payloads fall back to %v.
func customErrorMessage(tsv types.TypedSchemaValue) string {
	nodes := tsv.Value.ValueNodes
	root := int(tsv.Value.Root)
	if root >= 0 && root < len(nodes) && nodes[root].Tag() == types.SchemaValueNodeStringValue {
		return nodes[root].StringValue()
	}
	return fmt.Sprintf("%v", tsv)
}
