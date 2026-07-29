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

// A transaction runs a sequence of compensable [Operation]s so that if a later
// step fails, the steps that already succeeded are rolled back by running their
// compensations in reverse order. It builds on the durability primitives (atomic
// region + oplog) — no persistence beyond what the runtime already provides.
//
// Two flavors:
//
//   - [FallibleTransaction] rolls back on a step error and RETURNS a
//     [TransactionFailure] (best-effort; reports complete vs partial rollback).
//   - [InfallibleTransaction] rolls back, rewinds the oplog to the start, and the
//     runtime REPLAYS the whole transaction until it succeeds.
//
// Both share the [Operation]/[Step] building blocks. The E type parameter is the
// common error type across a transaction's steps (often string). Model expected
// step failures as the error arm of the step's [Result]; a panic (an infra
// failure) leaves the atomic region open so the runtime replays it, matching the
// fail-loud model used elsewhere in the SDK.

// Operation is a compensable step: an action paired with the rollback to undo it.
// compensate receives both the original input and the output execute produced, and
// runs only if a later step in the same transaction fails. Build one with
// [NewOperation].
type Operation[In, Out, E any] struct {
	execute    func(In) Result[Out, E]
	compensate func(In, Out) Result[Unit, E]
}

// NewOperation builds an [Operation] from an execute action and its compensation.
// execute reports an expected failure as the error arm of its [Result]; compensate
// should be best-effort and idempotent — it may run during rollback.
func NewOperation[In, Out, E any](
	execute func(In) Result[Out, E],
	compensate func(In, Out) Result[Unit, E],
) Operation[In, Out, E] {
	return Operation[In, Out, E]{execute: execute, compensate: compensate}
}

// Transaction accumulates the compensations of the steps that have succeeded so
// far. Obtain one from [FallibleTransaction] or [InfallibleTransaction] and pass
// it to [Step]; do not construct it directly.
type Transaction[E any] struct {
	compensations []func() Result[Unit, E]
}

// compensate runs the recorded compensations in reverse order and returns the
// first failure seen (None if all succeeded). Pure — no host interaction.
func (tx *Transaction[E]) compensate() Option[E] {
	for i := len(tx.compensations) - 1; i >= 0; i-- {
		if r := tx.compensations[i](); r.IsErr() {
			return Some(r.Err())
		}
	}
	return None[E]()
}

// Step runs op against input inside tx. On success it records op's compensation
// (to be run if the transaction later rolls back) and returns the Ok result; on
// failure it returns the error result — return it from the transaction body to
// trigger rollback. It is a free function because Go methods cannot take type
// parameters, mirroring [MethodDef.Call].
func Step[In, Out, E any](tx *Transaction[E], op Operation[In, Out, E], input In) Result[Out, E] {
	r := op.execute(input)
	if r.IsErr() {
		return r
	}
	out := r.Ok()
	tx.compensations = append(tx.compensations, func() Result[Unit, E] {
		return op.compensate(input, out)
	})
	return r
}

// TransactionFailure is the error arm of a [FallibleTransaction]: the step failure
// that aborted it, plus whether the rollback itself fully succeeded.
type TransactionFailure[E any] struct {
	// Error is the step failure that aborted the transaction.
	Error E
	// CompensationFailure is Some when a compensation itself failed during
	// rollback (a partial rollback); None means every compensation succeeded (a
	// complete rollback).
	CompensationFailure Option[E]
}

// RolledBackFully reports whether every compensation succeeded (a complete
// rollback). When false, some effects may remain and need manual attention.
func (f TransactionFailure[E]) RolledBackFully() bool {
	return f.CompensationFailure.IsNone()
}

// FallibleTransaction runs f as a transaction. If f returns Ok, the transaction
// commits and its value is returned. If f returns an error, the recorded
// compensations run in reverse (best-effort) and a [TransactionFailure] is
// returned — there is no retry. A panic in f (an infra failure) leaves the atomic
// region open so the runtime replays it.
func FallibleTransaction[Out, E any](f func(*Transaction[E]) Result[Out, E]) Result[Out, TransactionFailure[E]] {
	begin := apiHost.MarkBeginOperation()
	out := runFallible(&Transaction[E]{}, f)
	apiHost.MarkEndOperation(begin)
	return out
}

// runFallible is the host-free core of [FallibleTransaction]: run the body, roll
// back on error, assemble the result. Split out so it is natively testable.
func runFallible[Out, E any](tx *Transaction[E], f func(*Transaction[E]) Result[Out, E]) Result[Out, TransactionFailure[E]] {
	r := f(tx)
	if r.IsErr() {
		return Err[Out, TransactionFailure[E]](TransactionFailure[E]{
			Error:               r.Err(),
			CompensationFailure: tx.compensate(),
		})
	}
	return Ok[Out, TransactionFailure[E]](r.Ok())
}

// InfallibleTransaction runs f as a transaction that retries until it succeeds. If
// f returns Ok, its value is returned. If f returns an error, the recorded
// compensations run in reverse, the oplog is rewound to the transaction start, and
// the runtime replays the whole transaction. Use it only when the steps are
// expected to eventually succeed — a deterministic failure would retry forever.
func InfallibleTransaction[Out, E any](f func(*Transaction[E]) Result[Out, E]) Out {
	begin := apiHost.MarkBeginOperation()
	txBegin := apiHost.GetOplogIndex()

	tx := &Transaction[E]{}
	r := f(tx)
	if r.IsErr() {
		if compFail := tx.compensate(); compFail.IsSome() {
			// A compensation failed while rolling back for a retry — the
			// transaction cannot be made consistent, so fail loudly.
			panic("golem: compensation failed during an infallible transaction rollback")
		}
		// Rewind to the start; this interrupts the invocation and the runtime
		// replays the transaction, so it does not return.
		apiHost.SetOplogIndex(txBegin)
		panic("golem: unreachable after oplog rewind")
	}

	apiHost.MarkEndOperation(begin)
	return r.Ok()
}
