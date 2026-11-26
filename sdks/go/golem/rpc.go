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
	"time"

	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	clock "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_clocks_0_3_0_system_clock"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Cross-agent calls hang off the method descriptor rather than the client:
//
//	res, err := Charge.Call(pay, ChargeIn{AmountCents: 500})
//
// Go methods cannot introduce type parameters, so a Client[Id] could never have
// a Call[In, Out] method. The descriptor already binds In and Out, and shares Id
// with the client — which is what makes aiming a method at the wrong agent a
// compile error.
//
// A remote failure — the callee failed, or was unreachable after the runtime
// exhausted its retries, or returned a value that does not decode as Out — is
// returned as a [RemoteCallError], and so is an input that does not encode as
// the callee's parameters, before anything is sent. The runtime handles
// transient failures and replays deterministically, so a failure that reaches
// the guest is terminal.
// Each call form has a Must variant ([MethodDef.MustCall],
// [MethodDef.MustTrigger], …) that panics instead; a panic in an agent method
// traps the component, as in Rust, so the worker fails. Model *expected*
// outcomes as a [Result] in the method's output.
//
// Misuse — a zero Client, a stream on a Trigger or a Schedule — panics in every
// form.

// Scope cards.
//
// An awaited call — Call or MustCall — may carry a [ScopeCard] from
// [DeriveScope]. The callee holds the card's grants for that invocation only,
// and loses them when the invocation ends. Attaching a card lends it: the
// caller keeps it and may attach it again within the same invocation. Trigger
// and Schedule never carry a scope card, as the host accepts one only on an
// awaited call.
//
//	scope := golem.MustDeriveScope(bounds, parent)
//	out, err := Charge.WithScopeCard(scope).Call(pay, ChargeIn{AmountCents: 500})
//
// [Client.WithScopeCard] sets a default for every awaited call through the
// client; a card attached to the call takes precedence.
//
// A scope card is valid only during the invocation that derived it. Keeping
// one, or a client whose default is one, for a later invocation makes that
// invocation's awaited calls fail: the host retires the card when its
// invocation ends, and denies them. Releasing a retired card is safe.
//
// The scope card is checked before the input is encoded: a released scope card
// panics with [ErrPermissionCardMoved] and leaves every card in the input with
// the caller.

// ScopedMethod is a method descriptor with a scope card attached to its
// awaited calls, made by [MethodDef.WithScopeCard].
type ScopedMethod[Id, In, Out any] struct {
	m    MethodDef[Id, In, Out]
	card ScopeCard
}

// WithScopeCard attaches card to the awaited calls of m. The card is lent, not
// moved, and is valid only in the invocation that derived it. It panics on the
// zero ScopeCard.
func (m MethodDef[Id, In, Out]) WithScopeCard(card ScopeCard) ScopedMethod[Id, In, Out] {
	return ScopedMethod[Id, In, Out]{m: m, card: mustScopeCard(m.name, card)}
}

// Call is [MethodDef.Call] carrying the scope card.
func (s ScopedMethod[Id, In, Out]) Call(c Client[Id], in In) (Out, error) {
	return s.m.call(c, in, s.card)
}

// MustCall is [MethodDef.MustCall] carrying the scope card.
func (s ScopedMethod[Id, In, Out]) MustCall(c Client[Id], in In) Out {
	return Must(s.m.call(c, in, s.card))
}

// Call invokes the method and waits for its result. A remote failure is
// returned as a [RemoteCallError]; a call that the target withdrew before it
// started is one of kind [RemoteCancelled].
//
// The call goes through the asynchronous import, as every other SDK's does,
// which is the form the executor resumes into after suspending a caller that
// waited past its RPC idle window. A call that outlasts that window therefore
// suspends the worker and resumes it with the result, rather than holding it
// resident.
//
// While it waits, other goroutines of the same invocation may run: Call is a
// yield point. To have several calls in flight at once, make each in its own
// goroutine. A single target instance handles one invocation at a time, so
// calls to the same agent instance do not run in parallel; concurrency is
// across different targets.
//
// An awaited call cannot be withdrawn by abandoning it: Go cannot cancel one
// call in flight, so the goroutine that makes it waits for its result. Use
// [MethodDef.Trigger] and [Invocation.Cancel] for work that may need to be
// withdrawn before it starts.
func (m MethodDef[Id, In, Out]) Call(c Client[Id], in In) (Out, error) {
	return m.call(c, in, ScopeCard{})
}

// MustCall is [MethodDef.Call] that panics on a remote failure.
func (m MethodDef[Id, In, Out]) MustCall(c Client[Id], in In) Out {
	return Must(m.call(c, in, ScopeCard{}))
}

// call makes an awaited call carrying scope, or the client's default scope
// card when scope is the zero ScopeCard.
func (m MethodDef[Id, In, Out]) call(c Client[Id], in In, scope ScopeCard) (Out, error) {
	var zero Out
	if c.rpc == nil {
		panic(fmt.Errorf("golem: %s: called on a zero Client", m.name))
	}
	tree, card, err := m.awaitedCallArgs(c, in, scope)
	if err != nil {
		return zero, err
	}
	res := c.rpc.InvokeAndAwait(m.name, tree, card)
	if res.IsErr() {
		return zero, rpcErrorToGo(c.target(), m.name, res.Err())
	}
	return decodeOutput[Out](c.target(), m.name, res.Ok().Result)
}

// Trigger starts the method without waiting for its result, and returns the
// handle of the invocation once the target has durably accepted it. A failure
// to start the invocation is returned as a [RemoteCallError]; the handle's
// [Invocation.AwaitResult] reports how the invocation ended.
//
// A Trigger never carries a scope card, and it panics for a method that takes
// or returns a stream.
func (m MethodDef[Id, In, Out]) Trigger(c Client[Id], in In) (*Invocation[Out], error) {
	if c.rpc == nil {
		panic(fmt.Errorf("golem: %s: called on a zero Client", m.name))
	}
	m.refuseStreams("Trigger")
	tree, err := m.encodeInput(c.target(), in)
	if err != nil {
		return nil, err
	}
	res := c.rpc.Trigger(m.name, tree)
	if res.IsErr() {
		return nil, rpcErrorToGo(c.target(), m.name, res.Err())
	}
	return typedInvocation[Out](res.Ok(), m.name), nil
}

// MustTrigger is [MethodDef.Trigger] that panics on a remote failure.
func (m MethodDef[Id, In, Out]) MustTrigger(c Client[Id], in In) *Invocation[Out] {
	return Must(m.Trigger(c, in))
}

// Schedule arranges for the method to be invoked at the given time, and
// returns the handle of the invocation once the target has durably accepted
// it. The handle's [Invocation.Cancel] withdraws the invocation until it
// starts. A failure to schedule is returned as a [RemoteCallError].
//
// A Schedule never carries a scope card, and it panics for a method that takes
// or returns a stream.
func (m MethodDef[Id, In, Out]) Schedule(c Client[Id], at time.Time, in In) (*Invocation[Out], error) {
	if c.rpc == nil {
		panic(fmt.Errorf("golem: %s: called on a zero Client", m.name))
	}
	m.refuseStreams("Schedule")
	tree, err := m.encodeInput(c.target(), in)
	if err != nil {
		return nil, err
	}
	res := c.rpc.Schedule(instantFrom(at), m.name, tree)
	if res.IsErr() {
		return nil, rpcErrorToGo(c.target(), m.name, res.Err())
	}
	return typedInvocation[Out](res.Ok(), m.name), nil
}

// MustSchedule is [MethodDef.Schedule] that panics on a remote failure.
func (m MethodDef[Id, In, Out]) MustSchedule(c Client[Id], at time.Time, in In) *Invocation[Out] {
	return Must(m.Schedule(c, at, in))
}

// Invocation rebuilds the handle of an invocation of the method from its ID,
// as an earlier [MethodDef.Trigger] or [MethodDef.Schedule] returned it, for
// example in an earlier invocation of the calling agent. It makes no remote
// call: the ID only locates the invocation, and the target authorizes every
// operation on the handle.
func (m MethodDef[Id, In, Out]) Invocation(id InvocationID) *Invocation[Out] {
	m.refuseStreams("Invocation")
	return newInvocation(link.GetInvocationOps(invocationIDToWit(id)), m.name, decodeOutput[Out])
}

// typedInvocation is the handle of an invocation a typed client started.
func typedInvocation[Out any](raw *host.Invocation, method string) *Invocation[Out] {
	return newInvocation(link.HostInvocationOps(raw), method, decodeOutput[Out])
}

// ---------------------------------------------------------------------------
// encoding / decoding, shared by every call shape
// ---------------------------------------------------------------------------

// awaitedCallArgs resolves an awaited call's scope card — scope, or the
// client's default when scope is the zero ScopeCard — and then encodes the
// input. The order matters: a released scope card panics before any card in
// the input moves.
func (m MethodDef[Id, In, Out]) awaitedCallArgs(c Client[Id], in In, scope ScopeCard) (types.SchemaValueTree, witTypes.Option[*types.PermissionCard], error) {
	if scope.sst == nil {
		scope = c.scopeCard
	}
	card := scopeCardHandle(scope)
	tree, err := m.encodeInput(c.target(), in)
	return tree, card, err
}

// encodeInput encodes the In value into a parameter-list value tree. The callee
// derives the same field list from the same Go type, which is what keeps the two
// sides symmetric. A value that does not encode is a [RemoteProtocolError] of
// the call to target.
func (m MethodDef[Id, In, Out]) encodeInput(target string, in In) (types.SchemaValueTree, error) {
	inType := reflect.TypeFor[In]()
	if problem := engine.ParamsTypeProblem("In", inType); problem != "" {
		panic(fmt.Errorf("golem: %s: %s", m.name, problem))
	}
	tree, err := tryEncodeParams(defs.StructFields(inType), reflect.ValueOf(&in).Elem())
	if err != nil {
		return tree, &RemoteCallError{
			Target: target, Method: m.name, Kind: RemoteProtocolError,
			Message: "the input does not encode: " + err.Error(),
		}
	}
	return tree, nil
}

// decodeOutput decodes a remote result through the LOCAL descriptor's Out codec.
// That codec checks every node tag, so a remote returning the wrong shape yields
// an error rather than a panic. The permission cards in the result that the
// decode does not keep go back to the host.
func decodeOutput[Out any](target, method string, out witTypes.Option[types.SchemaValueTree]) (Out, error) {
	var zero Out
	outType := reflect.TypeFor[Out]()
	if outType == reflect.TypeFor[Unit]() {
		if out.IsSome() {
			engine.ReleaseCardsIn(out.Some().ValueNodes, releaseCard)
		}
		return zero, nil
	}
	if out.IsNone() {
		return zero, &RemoteCallError{
			Target: target, Method: method, Kind: RemoteProtocolError,
			Message: "remote returned no value for a non-unit output",
		}
	}
	tree := out.Some()
	dst := reflect.New(outType).Elem()
	dec := engine.Decoder{Nodes: tree.ValueNodes}
	if err := defs.Compile(outType).Decode(&dec, dst, tree.Root); err != nil {
		dec.ReleaseCards(releaseCard)
		return zero, &RemoteCallError{
			Target: target, Method: method, Kind: RemoteProtocolError,
			Message: "remote returned an undecodable value: " + err.Error(),
		}
	}
	dec.ReleaseRest(releaseCard)
	return dst.Interface().(Out), nil
}

// ---------------------------------------------------------------------------
// invocation identity and scheduling
// ---------------------------------------------------------------------------

// InvocationID identifies one invocation: the agent that runs it and a UUID.
// Its text form, from String, is <agent-id>.<uuid>, where both parts are
// UUIDs.
type InvocationID struct {
	AgentID AgentID
	UUID    UUID
}

// String returns the text form <agent-id>.<uuid>.
func (id InvocationID) String() string { return id.AgentID.String() + "." + id.UUID.String() }

// ParseInvocationID reads the text form of an [InvocationID]: two hyphenated
// UUIDs separated by a ".".
func ParseInvocationID(s string) (InvocationID, error) {
	invalid := func(reason string) (InvocationID, error) {
		return InvocationID{}, fmt.Errorf("golem: invalid invocation id %q: %s", s, reason)
	}
	const uuidLen = 36
	if len(s) != 2*uuidLen+1 || s[uuidLen] != '.' {
		return invalid("expected <agent-id>.<uuid>")
	}
	agent, err := ParseUUID(s[:uuidLen])
	if err != nil {
		return invalid(err.Error())
	}
	uuid, err := ParseUUID(s[uuidLen+1:])
	if err != nil {
		return invalid(err.Error())
	}
	return InvocationID{AgentID: AgentIDFromUUID(agent), UUID: uuid}, nil
}

// InvocationMetadata is the identity the host allocated for one remote
// invocation. For an ephemeral target, AgentID is the ID of the one-shot
// instance that ran it.
type InvocationMetadata struct {
	AgentID      AgentID
	InvocationID InvocationID
}

func invocationMetadataFrom(m host.InvocationMetadata) InvocationMetadata {
	agentID := agentIDFromWit(m.AgentId)
	return InvocationMetadata{
		AgentID:      agentID,
		InvocationID: InvocationID{AgentID: agentID, UUID: uuidFromWit(m.InvocationId.Id)},
	}
}

// invocationIDFromWit reads the ID of an agent invocation. An agent method
// invocation always belongs to an agent, so another owner is a broken host.
func invocationIDFromWit(id types.InvocationId) InvocationID {
	if id.Owner.Tag() != types.InvocationOwnerAgent {
		panic(fmt.Errorf("golem: the host returned an agent invocation that a tool owns (%q)", id.Owner.Tool()))
	}
	return InvocationID{AgentID: agentIDFromWit(id.Owner.Agent()), UUID: uuidFromWit(id.Id)}
}

func invocationIDToWit(id InvocationID) types.InvocationId {
	return types.InvocationId{
		Owner: types.MakeInvocationOwnerAgent(agentIDToWit(id.AgentID)),
		Id:    uuidToWit(id.UUID),
	}
}

func instantFrom(t time.Time) clock.Instant {
	return clock.Instant{
		Seconds:     t.Unix(),
		Nanoseconds: uint32(t.Nanosecond()),
	}
}

// ---------------------------------------------------------------------------
// errors
// ---------------------------------------------------------------------------

// RemoteCallErrorKind classifies an RPC failure. The kinds are the host's
// rpc-error cases.
type RemoteCallErrorKind uint8

const (
	// RemoteProtocolError is a transport or encoding level failure.
	RemoteProtocolError RemoteCallErrorKind = iota
	// RemoteDenied means the caller is not permitted to make this call.
	RemoteDenied
	// RemoteNotFound means the target agent or method does not exist: an
	// unknown agent ID, or the ID of an agent of another type, is not found.
	RemoteNotFound
	// RemoteInternalError is an unexpected failure on the remote side.
	RemoteInternalError
	// RemoteAgentError means the remote returned a domain error; see Cause.
	RemoteAgentError
	// RemoteConflict means a keyed create found an agent that already has the
	// key, or the agent at the ID that a key derives has another key.
	RemoteConflict
	// RemoteCancelled means the awaited invocation was withdrawn before it
	// started.
	RemoteCancelled
	// RemoteCannotCancelRunning means a cancellation found the invocation
	// already running, which cannot be withdrawn.
	RemoteCannotCancelRunning
)

func (k RemoteCallErrorKind) String() string {
	switch k {
	case RemoteCancelled:
		return "cancelled"
	case RemoteCannotCancelRunning:
		return "cannot cancel running"
	case RemoteDenied:
		return "denied"
	case RemoteNotFound:
		return "not found"
	case RemoteInternalError:
		return "remote internal error"
	case RemoteAgentError:
		return "remote agent error"
	case RemoteConflict:
		return "conflict"
	default:
		return "protocol error"
	}
}

// RemoteCallError is returned by every cross-agent call that fails, and by
// the client constructors when the host refuses them. Target is the agent's ID
// when the client knows it, and otherwise the name of its agent type.
//
// A remote domain error keeps its Cause rather than being flattened into a
// string, so a remote custom-error stays inspectable by the caller.
type RemoteCallError struct {
	Target  string
	Method  string
	Kind    RemoteCallErrorKind
	Message string
	Cause   error
}

func (e *RemoteCallError) Error() string {
	target := e.Target
	if target == "" {
		target = "<unknown agent>"
	}
	return fmt.Sprintf("golem: calling %s.%s: %s: %s", target, e.Method, e.Kind, e.Message)
}

func (e *RemoteCallError) Unwrap() error { return e.Cause }

func rpcErrorToGo(target, method string, e host.RpcError) error {
	err := &RemoteCallError{Target: target, Method: method}
	switch e.Tag() {
	case host.RpcErrorProtocolError:
		err.Kind, err.Message = RemoteProtocolError, e.ProtocolError()
	case host.RpcErrorDenied:
		err.Kind, err.Message = RemoteDenied, e.Denied()
	case host.RpcErrorNotFound:
		err.Kind, err.Message = RemoteNotFound, e.NotFound()
	case host.RpcErrorRemoteInternalError:
		err.Kind, err.Message = RemoteInternalError, e.RemoteInternalError()
	case host.RpcErrorRemoteAgentError:
		remote := e.RemoteAgentError()
		err.Kind = RemoteAgentError
		err.Cause = agentErrorToGo(remote)
		err.Message = err.Cause.Error()
	case host.RpcErrorConflict:
		err.Kind, err.Message = RemoteConflict, e.Conflict()
	case host.RpcErrorCancelled:
		err.Kind, err.Message = RemoteCancelled, e.Cancelled()
	case host.RpcErrorCannotCancelRunning:
		err.Kind, err.Message = RemoteCannotCancelRunning, e.CannotCancelRunning()
	default:
		panic(fmt.Sprintf("golem: unhandled rpc-error case %d", e.Tag()))
	}
	return err
}

// refuseStreams rejects the invocation handle forms for a method that carries a
// stream. A trigger and a schedule return before the call completes, so neither
// can hand the caller a stream endpoint — including when the only stream is in
// the output. Await the call instead.
func (m MethodDef[Id, In, Out]) refuseStreams(form string) {
	if defs.CarriesStream(reflect.TypeFor[In]()) || defs.CarriesStream(reflect.TypeFor[Out]()) {
		panic(fmt.Errorf(
			"golem: %s: %s cannot carry a stream; await the call with Call instead",
			m.name, form))
	}
}
