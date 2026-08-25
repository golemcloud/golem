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

	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
	apiOplog "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_oplog"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	durability "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_durability_durability"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Durability helpers expose Golem's exactly-once execution knobs to a handler:
// atomic regions, idempotence mode, an idempotency-key generator, and oplog
// commit. They wrap host functions that the runtime already implements; the
// durability semantics are guaranteed by the executor, and these are thin,
// fail-loud wrappers (a host failure traps and surfaces as an agent-error,
// matching the RPC/promise surface — no in-band error return).
//
// Concurrency: these knobs apply at the worker level — the scope is per worker,
// not per goroutine.
// Golem runs an agent single-threaded with cooperative task-switching only at
// await points (RPC, promise, sleep), so an atomic region or a
// WithIdempotenceMode scope is safe when it does not await while other goroutines
// run concurrently; nesting on a single logical flow is fine. To keep a scope from
// affecting concurrent work, don't hold it open across a concurrent await (e.g. a
// CallAsync fan-out); for concurrency, distribute work across agent instances.

// Atomically runs f as an atomic region: on normal return the region commits, so
// on a later replay it is treated as a single completed step. If f panics, the
// region is left open (MarkEndOperation is never reached) and the runtime
// re-executes the whole region on retry — all-or-nothing. Panicking is therefore
// the way to abort a region, consistent with the fail-loud RPC/promise model.
//
// Return a value by capturing it in an outer variable:
//
//	var total int64
//	golem.Atomically(func() { total = recompute() })
func Atomically(f func()) {
	begin := apiHost.MarkBeginOperation()
	f() // if this panics, the region stays open and is replayed on retry
	apiHost.MarkEndOperation(begin)
}

// WithIdempotenceMode sets the current idempotence mode and returns a function
// that restores the previous mode; use it with defer to scope it:
//
//	defer golem.WithIdempotenceMode(false)()
//
// The default is true — side effects are treated as idempotent and Golem gives
// at-least-once semantics. Setting it false gives at-most-once: the agent fails
// if it is unknown whether a side effect already ran.
func WithIdempotenceMode(idempotent bool) (restore func()) {
	prev := apiHost.GetIdempotenceMode()
	apiHost.SetIdempotenceMode(idempotent)
	return func() { apiHost.SetIdempotenceMode(prev) }
}

// GenerateIdempotencyKey returns a fresh idempotency key that is stable across
// replay — it is persisted and committed, so it can be handed to a third-party
// system (e.g. a payment processor) to make an external call idempotent.
func GenerateIdempotencyKey() UUID {
	return uuidFromWit(apiHost.GenerateIdempotencyKey())
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

// DurableFunctionType selects how the executor commits and replays a custom
// durable operation. Use ReadLocal/WriteLocal/ReadRemote/WriteRemote for the
// common cases; the batched and transaction variants are for libraries that
// coordinate a group of writes.
type DurableFunctionType struct{ raw apiOplog.WrappedFunctionType }

var (
	// ReadLocal reads local (worker-owned) state.
	ReadLocal = DurableFunctionType{apiOplog.MakeWrappedFunctionTypeReadLocal()}
	// WriteLocal writes local (worker-owned) state.
	WriteLocal = DurableFunctionType{apiOplog.MakeWrappedFunctionTypeWriteLocal()}
	// ReadRemote reads from an external system.
	ReadRemote = DurableFunctionType{apiOplog.MakeWrappedFunctionTypeReadRemote()}
	// WriteRemote writes to an external system (the usual choice for a side effect).
	WriteRemote = DurableFunctionType{apiOplog.MakeWrappedFunctionTypeWriteRemote()}
)

// WriteRemoteBatched marks a remote write that the executor may commit together
// with adjacent batched writes. Pass the oplog index that begins the batch to
// join an existing one; omit it to start a new batch.
func WriteRemoteBatched(begin ...uint64) DurableFunctionType {
	return DurableFunctionType{apiOplog.MakeWrappedFunctionTypeWriteRemoteBatched(optionalOplogIndex(begin))}
}

// WriteRemoteTransaction marks a remote write that participates in a durable
// transaction. Pass the oplog index that begins the transaction to join an
// existing one; omit it to start a new transaction.
func WriteRemoteTransaction(begin ...uint64) DurableFunctionType {
	return DurableFunctionType{apiOplog.MakeWrappedFunctionTypeWriteRemoteTransaction(optionalOplogIndex(begin))}
}

func optionalOplogIndex(begin []uint64) witTypes.Option[uint64] {
	if len(begin) > 0 {
		return witTypes.Some(begin[0])
	}
	return witTypes.None[uint64]()
}

// DurableSpec describes one custom durable operation: which function it stands
// for (Interface::Function, used as the persisted name) and its commit/replay
// policy. Set ForcedCommit to force an efficient oplog commit at the end of the
// operation.
type DurableSpec struct {
	Interface    string
	Function     string
	Type         DurableFunctionType
	ForcedCommit bool
}

// DurableOp wraps a non-durable side effect so it is recorded once and replayed
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
//   - Returning a value carries the outcome. If Out is a [Result], an err Result
//     is a RECORDED durable failure — it is persisted and replayed like any other
//     value, so the operation is not retried.
//   - Panicking is a transient defect: the unfinished invocation is dropped and
//     normal recovery re-executes body. Panic (or golem.Must) when the effect
//     should be retried rather than recorded.
//
// There is no async variant: a blocking body already suspends the fiber at its
// await points, so one function covers both cases.
func DurableOp[In any, Out any](spec DurableSpec, request In, body func() Out) Out {
	durability.ObserveFunctionCall(spec.Interface, spec.Function)

	name := spec.Function
	if spec.Interface != "" {
		name = spec.Interface + "::" + spec.Function
	}

	req := encodeDurableValue(reflect.ValueOf(&request).Elem())
	invocation := durability.BeginCustomDurableInvocation(name, req, spec.Type.raw)

	if invocation.Tag() == durability.CustomDurableInvocationReplayed {
		return decodeDurableValue[Out](name, invocation.Replayed().Response)
	}

	// Live: run the body, persist its result, and finish. If body panics the
	// invocation is left unfinished (dropped below) so recovery re-executes it.
	live := invocation.Live()
	committed := false
	defer func() {
		if !committed {
			live.Drop()
		}
	}()

	out := body()
	resp := encodeDurableValue(reflect.ValueOf(&out).Elem())
	durability.LiveCustomDurableInvocationFinish(live, resp, spec.ForcedCommit)
	committed = true
	return out
}

// encodeDurableValue encodes a single value (not a parameter list) into a
// self-describing typed schema value. It panics on failure, matching the
// fail-loud durability surface.
func encodeDurableValue(v reflect.Value) types.TypedSchemaValue {
	return types.TypedSchemaValue{
		Graph: defs.graphForType(v.Type()),
		Value: encodeWith(defs.compile(v.Type()), v),
	}
}

// decodeDurableValue decodes a replayed response back into Out, panicking on a
// mismatch (a replay divergence is not recoverable in-band).
func decodeDurableValue[Out any](name string, tv types.TypedSchemaValue) Out {
	typ := reflect.TypeFor[Out]()
	dst := reflect.New(typ).Elem()
	dec := decoder{nodes: tv.Value.ValueNodes}
	if err := defs.compile(typ).decode(&dec, dst, tv.Value.Root); err != nil {
		panic(fmt.Errorf("golem: durable %s: decoding replayed response: %w", name, err))
	}
	return dst.Interface().(Out)
}
