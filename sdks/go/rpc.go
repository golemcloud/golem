// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
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

	host "github.com/golemcloud/golem-go/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem-go/internal/wit/golem_core_types"
	clock "github.com/golemcloud/golem-go/internal/wit/wasi_clocks_0_3_0_system_clock"
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

// Call invokes the method and waits for its result.
//
// This maps to the synchronous `invoke-and-await` import, so it blocks the whole
// component until the remote returns. To have several calls in flight at once,
// use [MethodDef.CallAsync].
func (m MethodDef[Id, In, Out]) Call(c Client[Id], in In) (Out, error) {
	var zero Out
	if c.rpc == nil {
		return zero, fmt.Errorf("golem: %s: called on a zero Client", m.name)
	}

	tree, err := m.encodeInput(in)
	if err != nil {
		return zero, err
	}

	res := c.rpc.InvokeAndAwait(m.name, tree)
	if res.IsErr() {
		return zero, rpcErrorToGo(c.agentID, m.name, res.Err())
	}
	return decodeOutput[Out](c.agentID, m.name, res.Ok().Result)
}

// Trigger invokes the method without waiting for a result, returning the
// invocation's identity. Failures after the invocation is accepted are not
// reported here.
func (m MethodDef[Id, In, Out]) Trigger(c Client[Id], in In) (InvocationID, error) {
	if c.rpc == nil {
		return InvocationID{}, fmt.Errorf("golem: %s: called on a zero Client", m.name)
	}
	tree, err := m.encodeInput(in)
	if err != nil {
		return InvocationID{}, err
	}
	res := c.rpc.Invoke(m.name, tree)
	if res.IsErr() {
		return InvocationID{}, rpcErrorToGo(c.agentID, m.name, res.Err())
	}
	return invocationIDFrom(res.Ok()), nil
}

// Schedule arranges for the method to be invoked at the given time and returns a
// token that can cancel it beforehand.
func (m MethodDef[Id, In, Out]) Schedule(c Client[Id], at time.Time, in In) (*ScheduledInvocation, error) {
	if c.rpc == nil {
		return nil, fmt.Errorf("golem: %s: called on a zero Client", m.name)
	}
	tree, err := m.encodeInput(in)
	if err != nil {
		return nil, err
	}
	receipt := c.rpc.ScheduleCancelableInvocation(instantFrom(at), m.name, tree)
	return &ScheduledInvocation{
		ID:    invocationIDFrom(receipt.Metadata),
		token: receipt.CancellationToken,
	}, nil
}

// CallAsync starts the invocation and returns immediately with a future.
//
// This is the only way to have several invocations in flight: `invoke-and-await`
// is a synchronous import and blocks the component, whereas the future's Get is
// async, so a goroutine blocked in it yields to the component-model event loop
// and lets other goroutines proceed.
//
// Note the platform contract: a single target instance handles one invocation at
// a time, so fanning out to the SAME agent instance does not run in parallel.
// Concurrency is across DIFFERENT targets.
func (m MethodDef[Id, In, Out]) CallAsync(c Client[Id], in In) (*Future[Out], error) {
	if c.rpc == nil {
		return nil, fmt.Errorf("golem: %s: called on a zero Client", m.name)
	}
	tree, err := m.encodeInput(in)
	if err != nil {
		return nil, err
	}
	inv := c.rpc.AsyncInvokeAndAwait(m.name, tree)
	return &Future[Out]{
		ID:     invocationIDFrom(inv.Metadata),
		fut:    inv.Future,
		method: m.name,
		target: c.agentID,
	}, nil
}

// ---------------------------------------------------------------------------
// encoding / decoding, shared by every call shape
// ---------------------------------------------------------------------------

// rpcInputs caches the parameter field list per In type. The callee derives the
// same list from the same Go type, which is what keeps the two sides symmetric.
func (m MethodDef[Id, In, Out]) encodeInput(in In) (tree types.SchemaValueTree, err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("golem: %s: encoding input: %v", m.name, r)
		}
	}()
	inType := reflect.TypeFor[In]()
	return encodeParams(structFields(inType), reflect.ValueOf(&in).Elem()), nil
}

// decodeOutput decodes a remote result through the LOCAL descriptor's Out codec.
// That codec checks every node tag, so a remote returning the wrong shape yields
// an error rather than a panic.
func decodeOutput[Out any](target, method string, out witTypes.Option[types.SchemaValueTree]) (Out, error) {
	var zero Out
	outType := reflect.TypeFor[Out]()
	if outType == reflect.TypeFor[Unit]() {
		return zero, nil
	}
	if out.IsNone() {
		return zero, &RemoteCallError{
			Target: target, Method: method, Kind: RemoteProtocol,
			Msg: "remote returned no value for a non-unit output",
		}
	}
	tree := out.Some()
	dst := reflect.New(outType).Elem()
	d := decoder{nodes: tree.ValueNodes}
	if err := compile(outType).decode(&d, dst, tree.Root); err != nil {
		return zero, &RemoteCallError{
			Target: target, Method: method, Kind: RemoteProtocol,
			Msg: "remote returned an undecodable value: " + err.Error(),
		}
	}
	return dst.Interface().(Out), nil
}

// ---------------------------------------------------------------------------
// invocation identity and scheduling
// ---------------------------------------------------------------------------

// InvocationID identifies one remote invocation.
type InvocationID struct {
	AgentID        string
	IdempotencyKey string
}

func invocationIDFrom(m host.InvocationMetadata) InvocationID {
	return InvocationID{AgentID: m.AgentId, IdempotencyKey: m.IdempotencyKey}
}

// ScheduledInvocation is a future invocation that has not run yet.
type ScheduledInvocation struct {
	ID    InvocationID
	token *host.CancellationToken
}

// Cancel prevents the invocation, if it has not already started.
func (s *ScheduledInvocation) Cancel() {
	if s.token != nil {
		s.token.Cancel()
		s.token = nil
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

// RemoteErrorKind classifies an RPC failure, mirroring the WIT rpc-error cases.
type RemoteErrorKind uint8

const (
	// RemoteProtocol is a transport or encoding level failure.
	RemoteProtocol RemoteErrorKind = iota
	// RemoteDenied means the caller is not permitted to make this call.
	RemoteDenied
	// RemoteNotFound means the target agent or method does not exist.
	RemoteNotFound
	// RemoteInternal is an unexpected failure on the remote side.
	RemoteInternal
	// RemoteAgent means the remote returned a domain error; see Cause.
	RemoteAgent
)

func (k RemoteErrorKind) String() string {
	switch k {
	case RemoteDenied:
		return "denied"
	case RemoteNotFound:
		return "not found"
	case RemoteInternal:
		return "remote internal error"
	case RemoteAgent:
		return "remote agent error"
	default:
		return "protocol error"
	}
}

// RemoteCallError is returned by every cross-agent call that fails.
//
// A remote domain error keeps its Cause rather than being flattened into a
// string, so a remote custom-error stays inspectable by the caller.
type RemoteCallError struct {
	Target string
	Method string
	Kind   RemoteErrorKind
	Msg    string
	Cause  error
}

func (e *RemoteCallError) Error() string {
	target := e.Target
	if target == "" {
		target = "<unknown agent>"
	}
	return fmt.Sprintf("golem: calling %s.%s: %s: %s", target, e.Method, e.Kind, e.Msg)
}

func (e *RemoteCallError) Unwrap() error { return e.Cause }

func rpcErrorToGo(target, method string, e host.RpcError) error {
	err := &RemoteCallError{Target: target, Method: method}
	switch e.Tag() {
	case host.RpcErrorProtocolError:
		err.Kind, err.Msg = RemoteProtocol, e.ProtocolError()
	case host.RpcErrorDenied:
		err.Kind, err.Msg = RemoteDenied, e.Denied()
	case host.RpcErrorNotFound:
		err.Kind, err.Msg = RemoteNotFound, e.NotFound()
	case host.RpcErrorRemoteInternalError:
		err.Kind, err.Msg = RemoteInternal, e.RemoteInternalError()
	case host.RpcErrorRemoteAgentError:
		remote := e.RemoteAgentError()
		err.Kind = RemoteAgent
		err.Cause = agentErrorToGo(remote)
		err.Msg = err.Cause.Error()
	default:
		err.Kind, err.Msg = RemoteProtocol, fmt.Sprintf("unknown rpc-error (tag %d)", e.Tag())
	}
	return err
}
