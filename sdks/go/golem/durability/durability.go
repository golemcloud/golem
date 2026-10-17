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

// Package durability exposes idempotency keys, oplog commits, and custom
// durable operations.
package durability

import (
	"fmt"
	"reflect"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"

	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
	apiOplog "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_oplog"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witDurability "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_durability_durability"
)

// GenerateIdempotencyKey returns a fresh idempotency key that is stable across
// replay — it is persisted and committed, so it can be handed to a third-party
// system (e.g. a payment processor) to make an external call idempotent.
func GenerateIdempotencyKey() golem.UUID {
	return engine.UUIDFromWit(apiHost.GenerateIdempotencyKey())
}

// OplogCommit blocks until the oplog has been written to at least the given
// number of replicas (capped at the maximum available). Use it before a critical
// external effect to bound how much progress a crash could lose.
func OplogCommit(replicas uint8) {
	apiHost.OplogCommit(replicas)
}

// ---------------------------------------------------------------------------
// Custom durability
// ---------------------------------------------------------------------------

// FunctionType selects how the executor commits and replays a custom
// durable operation: ReadLocal, WriteLocal, ReadRemote, or WriteRemote.
type FunctionType struct{ raw apiOplog.WrappedFunctionType }

var (
	// ReadLocal reads local (worker-owned) state.
	ReadLocal = FunctionType{apiOplog.MakeWrappedFunctionTypeReadLocal()}
	// WriteLocal writes local (worker-owned) state.
	WriteLocal = FunctionType{apiOplog.MakeWrappedFunctionTypeWriteLocal()}
	// ReadRemote reads from an external system.
	ReadRemote = FunctionType{apiOplog.MakeWrappedFunctionTypeReadRemote()}
	// WriteRemote writes to an external system (the usual choice for a side effect).
	WriteRemote = FunctionType{apiOplog.MakeWrappedFunctionTypeWriteRemote()}
)

// Spec describes one custom durable operation: which function it stands
// for (Interface::Function, used as the persisted name) and its commit/replay
// policy. Set ForcedCommit to force an efficient oplog commit at the end of the
// operation.
type Spec struct {
	Interface    string
	Function     string
	Type         FunctionType
	ForcedCommit bool
}

// Run wraps a non-durable side effect so it is recorded once and replayed
// from the oplog thereafter. It is the building block for authoring custom
// durable operations (the SDK's own keyvalue/blobstore/http wrappers are built
// the same way).
//
// On a live run it executes body, persists the encoded result, and returns it;
// on replay it returns the recorded result WITHOUT running body. request is
// recorded alongside the result so the oplog is self-describing; both request
// and result are encoded through the ordinary schema codec.
//
// Failure has two distinct channels:
//   - Returning a value carries the outcome. If Out is a [golem.Result], an err Result
//     is a RECORDED durable failure — it is persisted and replayed like any other
//     value, so the operation is not retried.
//   - Panicking records nothing: the panic traps the component at once, even
//     where a caller above would recover it (such as a tool handler), which
//     fails the agent.
//
// There is no async variant: a blocking body already suspends the fiber at its
// await points, so one function covers both cases.
//
// Sequential and properly nested execution only. A blocking body can yield to
// other goroutines; the plain func signature does not isolate its custom scope.
// Do not let unrelated goroutines run while the operation is open, including
// ordinary host calls: sibling effects can acquire the wrong scope and cause
// incorrect replay or invocation failure. Misuse is not reliably detected.
// Complete each nested Run before continuing its parent. Do not detach goroutines
// or let work outlive the body, including on failure or cancellation. Stop and join
// outstanding work before the operation closes. Sequential blocking host calls
// remain supported when no unrelated goroutine runs during the wait.
func Run[In any, Out any](spec Spec, request In, body func() Out) Out {
	witDurability.ObserveFunctionCall(spec.Interface, spec.Function)

	name := spec.Function
	if spec.Interface != "" {
		name = spec.Interface + "::" + spec.Function
	}

	req := encodeDurableValue(reflect.ValueOf(&request).Elem())
	invocation := witDurability.BeginCustomDurableInvocation(name, req, spec.Type.raw)

	if invocation.Tag() == witDurability.CustomDurableInvocationReplayed {
		return decodeDurableValue[Out](name, invocation.Replayed().Response)
	}

	// Live: run the body, persist its result, and finish. A panic traps the
	// component at once, even where a caller above would recover it, so no
	// code runs on past an unfinished durable operation.
	live := invocation.Live()
	var out Out
	link.TrapOnPanic("durable operation "+name, func() {
		out = body()
		resp := encodeDurableValue(reflect.ValueOf(&out).Elem())
		witDurability.LiveCustomDurableInvocationFinish(live, resp, spec.ForcedCommit)
	})
	return out
}

// encodeDurableValue encodes a single value (not a parameter list) into a
// self-describing typed schema value. It panics on failure, matching the
// fail-loud durability surface.
func encodeDurableValue(v reflect.Value) types.TypedSchemaValue {
	return types.TypedSchemaValue{
		Graph: link.Engine.GraphForType(v.Type()),
		Value: engine.EncodeWith(link.Engine.Compile(v.Type()), v),
	}
}

// decodeDurableValue decodes a replayed response back into Out, panicking on a
// mismatch (a replay divergence is not recoverable in-band).
func decodeDurableValue[Out any](name string, tv types.TypedSchemaValue) Out {
	typ := reflect.TypeFor[Out]()
	dst := reflect.New(typ).Elem()
	dec := engine.Decoder{Nodes: tv.Value.ValueNodes}
	if err := link.Engine.Compile(typ).Decode(&dec, dst, tv.Value.Root); err != nil {
		panic(fmt.Errorf("golem: durable %s: decoding replayed response: %w", name, err))
	}
	return dst.Interface().(Out)
}
