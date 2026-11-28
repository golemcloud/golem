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

	"github.com/golemcloud/golem/sdks/go/golem/internal/identity"
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Client addresses one agent instance for remote calls. It is produced by
// [AgentDefinition.Upsert], [AgentDefinition.Create] or
// [AgentDefinition.GetByID], and consumed by the call methods on [MethodDef].
//
// The Id type parameter is what makes cross-agent calls type-safe: a
// MethodDef[PaymentID, …] only accepts a Client[PaymentID], so aiming a method
// at the wrong agent is a compile error rather than a runtime not-found.
//
// Client carries no type parameter for state: the caller does not know, and
// must not depend on, how the target stores anything.
type Client[Id any] struct {
	rpc       *host.WasmRpc
	agentType string
	// id is shared by the copies of a client, so the ID that AgentID learns
	// from the host is learned once.
	id        *clientID
	scopeCard ScopeCard
}

type clientID struct {
	id    AgentID
	known bool
}

func newClient[Id any](c identity.Connection) Client[Id] {
	id := &clientID{}
	if c.AgentID.IsSome() {
		id.id, id.known = agentIDFromWit(c.AgentID.Some()), true
	}
	return Client[Id]{rpc: c.RPC, agentType: c.TypeName, id: id}
}

// AgentID returns the ID of the client's agent. A client of Upsert asks the
// host for it the first time, and the host creates the agent when it is
// missing; the other clients already know it. A failure is a
// [RemoteCallError]. It panics on a zero Client.
func (c Client[Id]) AgentID() (AgentID, error) {
	if c.rpc == nil {
		panic(fmt.Errorf("golem: AgentID: called on a zero Client"))
	}
	if c.id.known {
		return c.id.id, nil
	}
	id, failure := identity.AgentID(identity.Connection{TypeName: c.agentType, AgentID: witTypes.None[types.AgentId](), RPC: c.rpc})
	if failure != nil {
		return AgentID{}, rpcErrorToGo(c.agentType, "<agent-id>", failure.RPCError)
	}
	c.id.id, c.id.known = agentIDFromWit(id), true
	return c.id.id, nil
}

// MustAgentID is [Client.AgentID] that panics on a failure.
func (c Client[Id]) MustAgentID() AgentID { return Must(c.AgentID()) }

// target names the client's agent in errors: its ID when the client knows it,
// and otherwise its agent type.
func (c Client[Id]) target() string {
	if c.id != nil && c.id.known {
		return c.id.id.String()
	}
	return c.agentType
}

// WithScopeCard returns a copy of the client whose awaited calls — Call and
// MustCall — carry card as their scope card, unless the call
// attaches its own with [MethodDef.WithScopeCard]. Trigger and Schedule never
// carry one. The card is lent, not moved: it must stay unreleased while the
// copy is used. It panics on the zero ScopeCard.
//
// A scope card is valid only in the invocation that derived it: use the copy
// within that invocation, and do not keep it for a later one, whose awaited
// calls the host would deny.
func (c Client[Id]) WithScopeCard(card ScopeCard) Client[Id] {
	c.scopeCard = mustScopeCard(c.agentType, card)
	return c
}

// CreateOpt configures the agent that a Create makes.
type CreateOpt func(*createOpts)

type createOpts struct {
	keyed   bool
	configs []configOverrideFn
	env     []witTypes.Tuple2[string, string]
}

// Keyed makes Create create the keyed agent of the constructor parameters: the
// agent that Upsert with the same parameters reaches. The create fails with a
// [RemoteCallError] of kind [RemoteConflict] when that agent already exists.
// Without Keyed, Create makes a new agent with a new ID and no key.
func Keyed() CreateOpt { return func(o *createOpts) { o.keyed = true } }

// WithEnv adds the environment variable name to the new agent's environment,
// next to the ones its manifest defines. The caller's own environment is not
// copied to the new agent.
func WithEnv(name, value string) CreateOpt {
	return func(o *createOpts) {
		o.env = append(o.env, witTypes.Tuple2[string, string]{F0: name, F1: value})
	}
}

// createOptions builds the host's create options for an agent registered as
// e. A config override that does not match the target's declarations is an
// error. Pure.
func createOptions(d *definitions, e *agentEntry, opts []CreateOpt) (host.CreateOptions, error) {
	var o createOpts
	for _, f := range opts {
		f(&o)
	}
	agentConfig, err := buildAgentConfig(d, e, o.configs)
	if err != nil {
		return host.CreateOptions{}, err
	}
	return host.CreateOptions{Keyed: o.keyed, AgentConfig: agentConfig, Env: o.env}, nil
}

// encodeID encodes the constructor parameters of the agent registered under
// name. An unknown name panics; an id that does not encode is an error.
func encodeID[Id any](d *definitions, op, name string, id Id) (*agentEntry, types.SchemaValueTree, error) {
	e := d.agents[name]
	if e == nil {
		panic(fmt.Errorf("golem: %s: unknown agent %s", op, name))
	}
	ctor, err := tryEncodeParams(e.idFields, reflect.ValueOf(&id).Elem())
	if err != nil {
		return e, ctor, fmt.Errorf("golem: %s %s: the id does not encode: %w", op, name, err)
	}
	return e, ctor, nil
}

// Upsert returns a client for the keyed agent of the constructor parameters id,
// or panics if the host refuses it. The host creates the agent when it is
// missing, without its configuration overrides; use [AgentDefinition.Create]
// with [Keyed] to give a new keyed agent overrides. For an ephemeral agent,
// each invocation runs on a new one-shot instance. The target is this
// component's own agent, so a refusal is a programming or configuration error
// with no in-band recovery; the panic traps the component, as in Rust.
//
// The id is encoded with the same codecs the target uses to decode its
// constructor parameters — they are derived from the same Go types — so caller
// and callee agree by construction rather than by convention.
func (a *AgentDefinition[Id, Cfg]) Upsert(id Id) Client[Id] {
	return Must(upsertClient[Id](defs, a.name, id))
}

// upsertClient builds a client of Upsert for an agent registered under name.
// It is shared by the local ([AgentDefinition.Upsert]) and client
// ([FullAgentClient.Upsert]) paths, which differ only in how the target was
// declared, never in how it is called.
func upsertClient[Id any](d *definitions, name string, id Id) (Client[Id], error) {
	_, ctor, err := encodeID(d, "Upsert", name, id)
	if err != nil {
		return Client[Id]{}, err
	}
	connected, failure := identity.Upsert(name, ctor)
	if failure != nil {
		return Client[Id]{}, rpcErrorToGo(name, "<upsert>", failure.RPCError)
	}
	return newClient[Id](connected), nil
}

// Create creates a durable agent with the constructor parameters id, and
// returns a client for it once its constructor has finished. Without [Keyed]
// the new agent has a new ID and no key; the client's [Client.AgentID] reads
// the ID. Its options set configuration overrides ([AgentDefinition.WithConfig],
// [WithConfigEntries]) and environment variables ([WithEnv]).
//
// A failure the host reports, such as the [RemoteConflict] of a keyed create,
// is a [RemoteCallError]. An ephemeral agent cannot be created: each of its
// invocations runs on a new instance, so it is addressed with Upsert only. An
// override that does not match the declared config panics.
func (a *AgentDefinition[Id, Cfg]) Create(id Id, opts ...CreateOpt) (Client[Id], error) {
	return createClient[Id](defs, a.name, id, opts)
}

// MustCreate is [AgentDefinition.Create] that panics on a failure.
func (a *AgentDefinition[Id, Cfg]) MustCreate(id Id, opts ...CreateOpt) Client[Id] {
	return Must(a.Create(id, opts...))
}

func createClient[Id any](d *definitions, name string, id Id, opts []CreateOpt) (Client[Id], error) {
	e, ctor, err := encodeID(d, "Create", name, id)
	if err != nil {
		return Client[Id]{}, err
	}
	if e.mode == common.AgentModeEphemeral {
		return Client[Id]{}, fmt.Errorf("golem: Create %s: an ephemeral agent is not created; address it with Upsert", name)
	}
	options, err := createOptions(d, e, opts)
	if err != nil {
		panic(fmt.Errorf("golem: Create %s: %w", name, err))
	}
	connected, failure := identity.Create(name, ctor, options)
	if failure != nil {
		return Client[Id]{}, rpcErrorToGo(name, "<create>", failure.RPCError)
	}
	return newClient[Id](connected), nil
}

// GetByID returns a client for the existing agent with the ID. It never creates
// the agent: an unknown ID, or the ID of an agent of another type, is a
// [RemoteCallError] of kind [RemoteNotFound]. An ephemeral agent's ID named one
// invocation, so it is refused.
func (a *AgentDefinition[Id, Cfg]) GetByID(id AgentID) (Client[Id], error) {
	return getClientByID[Id](defs, a.name, id)
}

// MustGetByID is [AgentDefinition.GetByID] that panics on a failure.
func (a *AgentDefinition[Id, Cfg]) MustGetByID(id AgentID) Client[Id] {
	return Must(a.GetByID(id))
}

func getClientByID[Id any](d *definitions, name string, id AgentID) (Client[Id], error) {
	if e := d.agents[name]; e != nil && e.mode == common.AgentModeEphemeral {
		return Client[Id]{}, fmt.Errorf("golem: GetByID %s: an ephemeral agent's ID cannot be addressed", name)
	}
	connected, failure := identity.GetByID(name, agentIDToWit(id))
	if failure != nil {
		return Client[Id]{}, rpcErrorToGo(id.String(), "<get-by-id>", failure.RPCError)
	}
	return newClient[Id](connected), nil
}

// LookupID returns the ID of the keyed agent of the constructor parameters id,
// or None when no such agent exists. It never creates the agent.
func (a *AgentDefinition[Id, Cfg]) LookupID(id Id) (Option[AgentID], error) {
	return lookupID[Id](defs, a.name, id)
}

func lookupID[Id any](d *definitions, name string, id Id) (Option[AgentID], error) {
	_, ctor, err := encodeID(d, "LookupID", name, id)
	if err != nil {
		return None[AgentID](), err
	}
	found, failure := identity.Lookup(name, ctor)
	if failure != nil {
		return None[AgentID](), rpcErrorToGo(name, "<lookup>", failure.RPCError)
	}
	if found.IsNone() {
		return None[AgentID](), nil
	}
	return Some(agentIDFromWit(found.Some())), nil
}

// BindSelf binds the current durable incarnation without creating it or waiting for initialization.
//
// Use this to schedule work on self from inside the constructor. Upsert reuses an
// initialized agent without rerunning its constructor, but waits for an unfinished
// constructor. Calling Upsert on self from that constructor would therefore deadlock.
//
// Scheduled work runs after construction finishes. This does not make awaited self-calls
// reentrant or bypass method permissions. A different agent type or an ephemeral owner
// returns an error. Replay preserves the recorded incarnation.
func (a *AgentDefinition[Id, Cfg]) BindSelf() (Client[Id], error) {
	return bindSelfClient[Id](a.name)
}

func bindSelfClient[Id any](name string) (Client[Id], error) {
	connected, failure := identity.BindSelf(name)
	if failure != nil {
		if failure.Stage == identity.TypeStage {
			return Client[Id]{}, agentErrorToGo(failure.AgentError)
		}
		return Client[Id]{}, rpcErrorToGo(name, "<bind-self>", failure.RPCError)
	}
	return newClient[Id](connected), nil
}

// getAnyClientByID returns a client for the existing agent with the ID, of the
// type its ID names.
func getAnyClientByID[Id any](id AgentID) (Client[Id], error) {
	connected, failure := identity.GetAnyByID(agentIDToWit(id))
	if failure != nil {
		return Client[Id]{}, rpcErrorToGo(id.String(), "<get-by-id>", failure.RPCError)
	}
	return newClient[Id](connected), nil
}

// AgentErrorKind classifies an [AgentError]. The kinds are the host's
// agent-error cases.
type AgentErrorKind uint8

const (
	// AgentInvalidInput means the input did not match the method's parameters.
	AgentInvalidInput AgentErrorKind = iota
	// AgentInvalidMethod means no such method exists on the agent.
	AgentInvalidMethod
	// AgentInvalidType means no such agent type exists.
	AgentInvalidType
	// AgentInvalidAgentID means the agent id does not parse, or does not match
	// the agent type's constructor.
	AgentInvalidAgentID
	// AgentCustomError means the agent reported a custom error.
	AgentCustomError
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
	case AgentInvalidAgentID:
		return "invalid agent id: " + e.Message
	case AgentCustomError:
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
	case common.AgentErrorInvalidAgentId:
		return &AgentError{Kind: AgentInvalidAgentID, Message: e.InvalidAgentId()}
	case common.AgentErrorCustomError:
		return &AgentError{Kind: AgentCustomError, Message: customErrorMessage(e.CustomError())}
	default:
		panic(fmt.Sprintf("golem: unhandled agent-error case %d", e.Tag()))
	}
}

// customErrorMessage recovers the string payload the SDK encodes into the
// custom-error case (see customError). Without this the TypedSchemaValue would
// render as a struct dump, and — on the RPC path — a remote agent's error message
// would reach the caller as that dump. Non-string payloads fall back to %v.
func customErrorMessage(tsv types.TypedSchemaValue) string {
	nodes := tsv.Value.ValueNodes
	root := int(tsv.Value.Root)
	if root >= 0 && root < len(nodes) && nodes[root].Tag() == types.SchemaValueNodeStringValue {
		return nodes[root].StringValue()
	}
	return fmt.Sprintf("%v", tsv)
}
