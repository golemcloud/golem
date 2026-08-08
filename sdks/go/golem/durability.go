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
	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
)

// Durability helpers expose Golem's exactly-once execution knobs to a handler:
// atomic regions, persistence level, idempotence mode, an idempotency-key
// generator, and oplog commit. They wrap host functions that the runtime already
// implements; the durability semantics are guaranteed by the executor, and these
// are thin, fail-loud wrappers (a host failure traps and surfaces as an
// agent-error, matching the RPC/promise surface — no in-band error return).
//
// Concurrency: these knobs apply at the worker level — the scope is currently per
// worker rather than per goroutine (the same model as the other Golem SDKs).
// Golem runs an agent single-threaded with cooperative task-switching only at
// await points (RPC, promise, sleep), so an atomic region or a
// WithPersistenceLevel/WithIdempotenceMode scope is safe when it does not await
// while other goroutines run concurrently; nesting on a single logical flow is
// fine. To keep a scope from affecting concurrent work, don't hold it open across
// a concurrent await (e.g. a CallAsync fan-out); for concurrency, distribute work
// across agent instances.

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

// PersistenceLevel controls how much of an agent's execution is written to the
// oplog. Lowering it speeds up execution where durable replay is not required.
type PersistenceLevel int

const (
	// PersistNothing does not persist side effects; such a zone is skipped on
	// replay. Use only for work that is safe to re-run or that you re-derive.
	PersistNothing PersistenceLevel = iota
	// PersistRemoteSideEffects persists remote side effects (e.g. RPC results).
	PersistRemoteSideEffects
	// PersistSmart is the default heuristic.
	PersistSmart
)

func (l PersistenceLevel) toWit() apiHost.PersistenceLevel {
	switch l {
	case PersistNothing:
		return apiHost.MakePersistenceLevelPersistNothing()
	case PersistRemoteSideEffects:
		return apiHost.MakePersistenceLevelPersistRemoteSideEffects()
	default:
		return apiHost.MakePersistenceLevelSmart()
	}
}

func persistenceLevelFromWit(w apiHost.PersistenceLevel) PersistenceLevel {
	switch w.Tag() {
	case apiHost.PersistenceLevelPersistNothing:
		return PersistNothing
	case apiHost.PersistenceLevelPersistRemoteSideEffects:
		return PersistRemoteSideEffects
	default:
		return PersistSmart
	}
}

// WithPersistenceLevel sets the current persistence level and returns a function
// that restores the previous level. Use it with defer to scope a level to the
// rest of the current function:
//
//	defer golem.WithPersistenceLevel(golem.PersistNothing)()
//
// Scope a sub-block by wrapping it in a function literal with its own defer.
func WithPersistenceLevel(level PersistenceLevel) (restore func()) {
	prev := persistenceLevelFromWit(apiHost.GetOplogPersistenceLevel())
	apiHost.SetOplogPersistenceLevel(level.toWit())
	return func() { apiHost.SetOplogPersistenceLevel(prev.toWit()) }
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
