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
	"github.com/golemcloud/golem/sdks/go/golem/internal/cdctx"
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
// The operation's custom scope belongs to the goroutine that calls Run. Other
// goroutines keep their own scope while the body blocks, so concurrent Run
// calls and unrelated host calls do not interfere. Goroutines started inside
// the body, directly or transitively, and the callbacks of time.AfterFunc
// timers that the body sets, inherit the operation's scope, and Run returns
// only after all of them have exited. If one of them never exits, Run does not
// return and the invocation fails without a result. Timers that the body set
// and that are still pending when it returns are stopped.
//
// A completed operation replays its result without running the body, so the
// rest of the program must observe the body only through its return value. The
// runtime keeps the operation's clock, timers and random state (map seeds and
// iteration order, select, math/rand) separate from the rest of the program.
// Heap addresses are not separated: code after the operation must not depend
// on pointer values, such as the iteration order of a map keyed by pointers.
func Run[In any, Out any](spec Spec, request In, body func() Out) Out {
	scope := cdctx.Begin()
	witDurability.ObserveFunctionCall(spec.Interface, spec.Function)

	name := spec.Function
	if spec.Interface != "" {
		name = spec.Interface + "::" + spec.Function
	}

	req := encodeDurableValue(reflect.ValueOf(&request).Elem())
	invocation := witDurability.BeginCustomDurableInvocationAsync(name, req, spec.Type.raw)

	if invocation.Tag() == witDurability.CustomDurableInvocationReplayed {
		out := decodeDurableValue[Out](name, invocation.Replayed().Response)
		cdctx.End(scope)
		return out
	}

	// Live: run the body, wait for the goroutines it started, persist its
	// result, and finish. A panic traps the component at once, even where a
	// caller above would recover it, so no code runs on past an unfinished
	// durable operation.
	live := invocation.Live()
	var out Out
	link.TrapOnPanic("durable operation "+name, func() {
		live.Enter()
		cdctx.Entered(scope)
		out = body()
		cdctx.Join(scope)
		resp := encodeDurableValue(reflect.ValueOf(&out).Elem())
		witDurability.LiveCustomDurableInvocationFinishAsync(live, resp, spec.ForcedCommit)
		live.Drop()
	})
	cdctx.End(scope)
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
