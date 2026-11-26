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

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Invocation is the handle of an accepted invocation of an agent method,
// returned by [MethodDef.Trigger] and [MethodDef.Schedule] once the target has
// durably accepted the invocation. [MethodDef.Invocation] rebuilds it from its
// ID, for example in a later invocation of the calling agent.
//
// The ID only locates the invocation; it is not authority. Only the caller that
// started the invocation, or a caller with the applicable management
// permission, may observe or cancel it: the target authorizes every operation,
// and denies the others with a [RemoteCallError] of kind [RemoteDenied].
//
// Dropping the handle, or a goroutine that waits in [Invocation.AwaitResult],
// only detaches the caller: the invocation keeps running. A trigger or a
// schedule never lends a scope card, and refuses a method that takes or
// returns a stream. A running invocation cannot be cancelled: [Invocation.Cancel]
// fails with [RemoteCannotCancelRunning].
//
// A handle is a handle, not data, so it cannot be sent: send its [Invocation.ID].
type Invocation[Out any] struct {
	id     InvocationID
	method string
	ops    link.InvocationOps
	decode func(witTypes.Option[types.SchemaValueTree]) (Out, error)
}

// invocationish recognises an Invocation of any result type, which the codec
// refuses.
type invocationish interface{ invocationHandle() }

func (Invocation[Out]) invocationHandle() {}

// newInvocation is the handle of the invocation of method that ops reach,
// whose result decode reads.
func newInvocation[Out any](ops link.InvocationOps, method string, decode func(target, method string, out witTypes.Option[types.SchemaValueTree]) (Out, error)) *Invocation[Out] {
	id := invocationIDFromWit(ops.ID)
	target := id.AgentID.String()
	return &Invocation[Out]{
		id:     id,
		method: method,
		ops:    ops,
		decode: func(out witTypes.Option[types.SchemaValueTree]) (Out, error) { return decode(target, method, out) },
	}
}

// ID returns the ID of the invocation. Store it to reach the invocation from a
// later invocation of this agent, through [MethodDef.Invocation].
func (i *Invocation[Out]) ID() InvocationID { return i.id }

// Status reads the current lifecycle state of the invocation. It fails with a
// [RemoteCallError] of kind [RemoteNotFound] when the invocation or its agent
// no longer exists.
func (i *Invocation[Out]) Status() (InvocationStatus, error) {
	res := i.ops.Status()
	if res.IsErr() {
		return 0, rpcErrorToGo(i.target(), i.method, res.Err())
	}
	return invocationStatusFromWit(res.Ok()), nil
}

// AwaitResult waits for the invocation to finish and returns its result,
// without starting, resuming or resubmitting it. A remote failure, or a result
// that does not decode as Out, is returned as a [RemoteCallError]; an
// invocation that was cancelled before it started fails with one of kind
// [RemoteCancelled]. Every call asks the host again, so several goroutines may
// await the same handle.
//
// A goroutine that stops waiting only detaches: Go cannot withdraw one awaited
// call, and the invocation keeps running in either case.
func (i *Invocation[Out]) AwaitResult() (Out, error) {
	res := i.ops.Await()
	if res.IsErr() {
		var zero Out
		return zero, rpcErrorToGo(i.target(), i.method, res.Err())
	}
	return i.decode(res.Ok())
}

// Cancel withdraws the invocation if it has not started yet, and returns true.
// It returns false when the invocation already finished or was already
// cancelled, and fails with a [RemoteCallError] of kind
// [RemoteCannotCancelRunning] when it is running: a running invocation cannot
// be cancelled.
func (i *Invocation[Out]) Cancel() (bool, error) {
	res := i.ops.Cancel()
	if res.IsErr() {
		return false, rpcErrorToGo(i.target(), i.method, res.Err())
	}
	return res.Ok(), nil
}

// target names the invocation's agent in errors.
func (i *Invocation[Out]) target() string { return i.id.AgentID.String() }

// InvocationStatus is the lifecycle state of an accepted invocation.
type InvocationStatus uint8

const (
	// InvocationPending is accepted and waiting for its turn.
	InvocationPending InvocationStatus = iota
	// InvocationScheduled is accepted and waiting for its scheduled time.
	InvocationScheduled
	// InvocationRunning is started and executing.
	InvocationRunning
	// InvocationWaiting is started and waiting, for example for a promise or a
	// timer.
	InvocationWaiting
	// InvocationRetrying is started and waiting to retry after a failure.
	InvocationRetrying
	// InvocationInterrupted is started, and its agent was interrupted by an
	// operator.
	InvocationInterrupted
	// InvocationCompleted finished with a result.
	InvocationCompleted
	// InvocationFailed finished with a failure.
	InvocationFailed
	// InvocationCancelled was withdrawn before it started.
	InvocationCancelled
	// InvocationBackgroundFailed recorded its result, then its agent failed
	// during the invocation's background work.
	InvocationBackgroundFailed
)

func (s InvocationStatus) String() string {
	switch s {
	case InvocationPending:
		return "pending"
	case InvocationScheduled:
		return "scheduled"
	case InvocationRunning:
		return "running"
	case InvocationWaiting:
		return "waiting"
	case InvocationRetrying:
		return "retrying"
	case InvocationInterrupted:
		return "interrupted"
	case InvocationCompleted:
		return "completed"
	case InvocationFailed:
		return "failed"
	case InvocationCancelled:
		return "cancelled"
	case InvocationBackgroundFailed:
		return "background-failed"
	default:
		return "unknown"
	}
}

func invocationStatusFromWit(s host.InvocationStatus) InvocationStatus {
	switch s {
	case host.InvocationStatusPending:
		return InvocationPending
	case host.InvocationStatusScheduled:
		return InvocationScheduled
	case host.InvocationStatusRunning:
		return InvocationRunning
	case host.InvocationStatusWaiting:
		return InvocationWaiting
	case host.InvocationStatusRetrying:
		return InvocationRetrying
	case host.InvocationStatusInterrupted:
		return InvocationInterrupted
	case host.InvocationStatusCompleted:
		return InvocationCompleted
	case host.InvocationStatusFailed:
		return InvocationFailed
	case host.InvocationStatusCancelled:
		return InvocationCancelled
	case host.InvocationStatusBackgroundFailed:
		return InvocationBackgroundFailed
	default:
		panic(fmt.Sprintf("golem: unhandled invocation-status case %d", s))
	}
}

func init() {
	link.AnyInvocation = func(ops link.InvocationOps, method string, decode func(witTypes.Option[types.SchemaValueTree]) (any, error)) any {
		return newInvocation(ops, method, func(_, _ string, out witTypes.Option[types.SchemaValueTree]) (any, error) { return decode(out) })
	}
	link.ValueInvocation = func(ops link.InvocationOps, method string, decode func(witTypes.Option[types.SchemaValueTree]) (Option[core.SchemaValue], error)) any {
		return newInvocation(ops, method, func(_, _ string, out witTypes.Option[types.SchemaValueTree]) (Option[core.SchemaValue], error) {
			return decode(out)
		})
	}
}
