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

package transaction

import (
	"fmt"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
)

// A transaction runs a sequence of compensable [Operation]s so that if a later
// step fails, the steps that already succeeded are rolled back by running their
// compensations in reverse order. It uses the runtime's ordinary durability.
//
// Two flavors:
//
//   - [Fallible] rolls back on a step error and RETURNS a
//     [Failure] (best-effort; reports complete vs partial rollback).
//   - [Infallible] rolls back and starts new attempts until one succeeds.
//
// Both share the [Operation]/[Tx.Step] building blocks. The E type parameter is the
// common error type across a transaction's steps (often string). Model expected
// step failures as the error arm of the step's [golem.Result]. Panics do not
// trigger compensation. Each transaction owns its own compensation list.

// Operation is a compensable step: an action paired with the rollback to undo it.
// compensate receives both the original input and the output execute produced, and
// runs only if a later step in the same transaction fails. Build one with
// [NewOperation].
type Operation[In, Out, E any] struct {
	execute    func(In) golem.Result[Out, E]
	compensate func(In, Out) golem.Result[golem.Unit, E]
}

// NewOperation builds an [Operation] from an execute action and its compensation.
// execute reports an expected failure as the error arm of its [golem.Result]; compensate
// should be best-effort and idempotent — it may run during rollback.
func NewOperation[In, Out, E any](
	execute func(In) golem.Result[Out, E],
	compensate func(In, Out) golem.Result[golem.Unit, E],
) Operation[In, Out, E] {
	return Operation[In, Out, E]{execute: execute, compensate: compensate}
}

// Tx accumulates the compensations of the steps that have succeeded so
// far. Obtain one from [Fallible] or [Infallible] and pass
// it to [Tx.Step]; do not construct it directly.
type Tx[E any] struct {
	compensations []func() golem.Result[golem.Unit, E]
}

// compensate runs the recorded compensations in reverse order and returns the
// first failure seen (None if all succeeded). Pure — no host interaction.
func (tx *Tx[E]) compensate() golem.Option[E] {
	for i := len(tx.compensations) - 1; i >= 0; i-- {
		if r := tx.compensations[i](); r.IsErr() {
			return golem.Some(r.Err())
		}
	}
	return golem.None[E]()
}

// Step runs op against input inside the transaction. On success it records op's
// compensation (to be run if the transaction later rolls back) and returns the
// Ok result; on failure it returns the error result — return it from the
// transaction body to trigger rollback.
func (tx *Tx[E]) Step[In, Out any](op Operation[In, Out, E], input In) golem.Result[Out, E] {
	r := op.execute(input)
	if r.IsErr() {
		return r
	}
	out := r.Ok()
	tx.compensations = append(tx.compensations, func() golem.Result[golem.Unit, E] {
		return op.compensate(input, out)
	})
	return r
}

// Failure is the error arm of a [Fallible]: the step failure
// that aborted it, plus whether the rollback itself fully succeeded.
type Failure[E any] struct {
	// Error is the step failure that aborted the transaction.
	Error E
	// CompensationFailure is Some when a compensation itself failed during
	// rollback (a partial rollback); None means every compensation succeeded (a
	// complete rollback).
	CompensationFailure golem.Option[E]
}

// RolledBackFully reports whether every compensation succeeded (a complete
// rollback). When false, some effects may remain and need manual attention.
func (f Failure[E]) RolledBackFully() bool {
	return f.CompensationFailure.IsNone()
}

// Fallible runs f as a transaction. If f returns golem.Ok, the transaction
// commits and its value is returned. If f returns an error, the recorded
// compensations run in reverse (best-effort) and a [Failure] is
// returned — there is no retry. A panic traps without triggering compensation.
func Fallible[Out, E any](f func(*Tx[E]) golem.Result[Out, E]) golem.Result[Out, Failure[E]] {
	var out golem.Result[Out, Failure[E]]
	link.TrapOnPanic("transaction", func() { out = runFallible(&Tx[E]{}, f) })
	return out
}

// runFallible is the host-free core of [Fallible]: run the body, roll
// back on error, assemble the result. Split out so it is natively testable.
func runFallible[Out, E any](tx *Tx[E], f func(*Tx[E]) golem.Result[Out, E]) golem.Result[Out, Failure[E]] {
	r := f(tx)
	if r.IsErr() {
		return golem.Err[Out, Failure[E]](Failure[E]{
			Error:               r.Err(),
			CompensationFailure: tx.compensate(),
		})
	}
	return golem.Ok[Out, Failure[E]](r.Ok())
}

// Infallible runs f as a transaction that retries until it succeeds. If
// f returns golem.Ok, its value is returned. If f returns an error, the recorded
// compensations run in reverse before starting a fresh attempt. Attempts append
// durable history and never rewind it. Use it only when the steps are
// expected to eventually succeed — a deterministic failure would retry forever.
// A compensation failure panics instead of starting another attempt.
func Infallible[Out, E any](f func(*Tx[E]) golem.Result[Out, E]) Out {
	var out Out
	link.TrapOnPanic("transaction", func() { out = runInfallible(f) })
	return out
}

func runInfallible[Out, E any](f func(*Tx[E]) golem.Result[Out, E]) Out {
	for {
		tx := &Tx[E]{}
		r := f(tx)
		if r.IsOk() {
			return r.Ok()
		}
		if compFail := tx.compensate(); compFail.IsSome() {
			panic(fmt.Errorf("golem: compensation failed during an infallible transaction rollback: %v", compFail.Unwrap()))
		}
	}
}
