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
	"errors"
	"testing"

	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

var testInvocationID = InvocationID{AgentID: testAgentID, UUID: testInvocationUUID}

// scriptedInvocation answers each operation of a handle as scripted.
type scriptedInvocation struct {
	status    witTypes.Result[host.InvocationStatus, host.RpcError]
	await     witTypes.Result[witTypes.Option[types.SchemaValueTree], host.RpcError]
	cancel    witTypes.Result[bool, host.RpcError]
	awaitedBy int
}

func (s *scriptedInvocation) ops() link.InvocationOps {
	return link.InvocationOps{
		ID:     invocationIDToWit(testInvocationID),
		Status: func() witTypes.Result[host.InvocationStatus, host.RpcError] { return s.status },
		Await: func() witTypes.Result[witTypes.Option[types.SchemaValueTree], host.RpcError] {
			s.awaitedBy++
			return s.await
		},
		Cancel: func() witTypes.Result[bool, host.RpcError] { return s.cancel },
	}
}

func remoteKind(t *testing.T, err error) RemoteCallErrorKind {
	t.Helper()
	var rce *RemoteCallError
	if !errors.As(err, &rce) {
		t.Fatalf("expected a *RemoteCallError, got %T %v", err, err)
	}
	if rce.Target != testAgentID.String() || rce.Method != "charge" {
		t.Fatalf("the error names %s.%s", rce.Target, rce.Method)
	}
	return rce.Kind
}

// An invocation ID names an agent invocation, and travels to the host and back
// unchanged.
func TestAnInvocationIDRoundTripsThroughTheHostForm(t *testing.T) {
	wit := invocationIDToWit(testInvocationID)
	if wit.Owner.Tag() != types.InvocationOwnerAgent {
		t.Fatalf("the owner is tag %d, want an agent", wit.Owner.Tag())
	}
	if got := invocationIDFromWit(wit); got != testInvocationID {
		t.Fatalf("round trip gave %+v, want %+v", got, testInvocationID)
	}
}

// A handle reports its ID, reads and decodes the result as the typed
// descriptor does, and asks the host again on every await.
func TestAnInvocationAwaitsAndDecodesItsResult(t *testing.T) {
	money := Money{Amount: 7, Currency: "EUR"}
	script := &scriptedInvocation{
		await: witTypes.Ok[witTypes.Option[types.SchemaValueTree], host.RpcError](witTypes.Some(encodeValue(money))),
	}
	inv := newInvocation(script.ops(), "charge", decodeOutput[Money])
	if inv.ID() != testInvocationID {
		t.Fatalf("ID() = %+v, want %+v", inv.ID(), testInvocationID)
	}
	for range 2 {
		if got, err := inv.AwaitResult(); err != nil || got != money {
			t.Fatalf("AwaitResult() = %+v, %v", got, err)
		}
	}
	if script.awaitedBy != 2 {
		t.Fatalf("the host was asked %d times, want 2", script.awaitedBy)
	}

	script.await = witTypes.Ok[witTypes.Option[types.SchemaValueTree], host.RpcError](witTypes.None[types.SchemaValueTree]())
	if _, err := inv.AwaitResult(); remoteKind(t, err) != RemoteProtocolError {
		t.Fatalf("a missing result gave %v", err)
	}
	script.await = witTypes.Err[witTypes.Option[types.SchemaValueTree]](host.MakeRpcErrorCancelled("withdrawn"))
	if _, err := inv.AwaitResult(); remoteKind(t, err) != RemoteCancelled {
		t.Fatalf("a cancelled invocation gave %v", err)
	}
}

// Cancel tells a withdrawn invocation from one that had already ended, and
// fails for a running one.
func TestAnInvocationCancelsOnlyBeforeItStarts(t *testing.T) {
	script := &scriptedInvocation{cancel: witTypes.Ok[bool, host.RpcError](true)}
	inv := newInvocation(script.ops(), "charge", decodeOutput[Unit])
	if ok, err := inv.Cancel(); !ok || err != nil {
		t.Fatalf("Cancel() = %v, %v; want true", ok, err)
	}
	script.cancel = witTypes.Ok[bool, host.RpcError](false)
	if ok, err := inv.Cancel(); ok || err != nil {
		t.Fatalf("a repeated Cancel() = %v, %v; want false", ok, err)
	}
	script.cancel = witTypes.Err[bool](host.MakeRpcErrorCannotCancelRunning("running"))
	if ok, err := inv.Cancel(); ok || remoteKind(t, err) != RemoteCannotCancelRunning {
		t.Fatalf("cancelling a running invocation gave %v, %v", ok, err)
	}
}

// Status reads each host state, and reports a failure as a RemoteCallError.
func TestAnInvocationReadsEveryStatus(t *testing.T) {
	script := &scriptedInvocation{}
	inv := newInvocation(script.ops(), "charge", decodeOutput[Unit])
	for wit, want := range map[host.InvocationStatus]InvocationStatus{
		host.InvocationStatusPending:          InvocationPending,
		host.InvocationStatusScheduled:        InvocationScheduled,
		host.InvocationStatusRunning:          InvocationRunning,
		host.InvocationStatusWaiting:          InvocationWaiting,
		host.InvocationStatusRetrying:         InvocationRetrying,
		host.InvocationStatusInterrupted:      InvocationInterrupted,
		host.InvocationStatusCompleted:        InvocationCompleted,
		host.InvocationStatusFailed:           InvocationFailed,
		host.InvocationStatusCancelled:        InvocationCancelled,
		host.InvocationStatusBackgroundFailed: InvocationBackgroundFailed,
	} {
		script.status = witTypes.Ok[host.InvocationStatus, host.RpcError](wit)
		if got, err := inv.Status(); got != want || err != nil {
			t.Errorf("status %d read as %v, %v; want %v", wit, got, err, want)
		}
		if want.String() == "unknown" {
			t.Errorf("%d has no name", want)
		}
	}
	script.status = witTypes.Err[host.InvocationStatus](host.MakeRpcErrorNotFound("gone"))
	if _, err := inv.Status(); remoteKind(t, err) != RemoteNotFound {
		t.Fatalf("a deleted invocation gave %v", err)
	}
}
