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
	"time"

	"github.com/golemcloud/golem/sdks/go/golem"
)

// A transaction runs a sequence of compensable [Operation]s so that if a later
// step fails, the steps that already succeeded are rolled back by running their
// compensations in reverse order. It uses the runtime's ordinary durability.
//
// Two flavors:
//
//   - [Fallible] rolls back on a step error and RETURNS a
//     [Failure] (best-effort; reports complete vs partial rollback).
//   - [Infallible] rolls back and retries within the host's semantic retry policy.
//
// Both share the [Operation]/[Tx.Step] building blocks. The E type parameter is the
// common error type across a transaction's steps (often string). Model expected
// step failures as the error arm of the step's [golem.Result]. Catchable panics
// also trigger compensation. Each transaction owns its own compensation list.

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
// first failure seen (None if all succeeded).
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

// Failure is the error arm of [Fallible] and [Infallible]: the step failure
// that aborted it, plus whether the rollback itself fully succeeded.
type Failure[E any] struct {
	// Panic preserves the original recovered value when the body panicked.
	Panic any
	// CompensationPanic preserves a panic from rollback, which stops rollback.
	CompensationPanic any
	// PolicyError is set when the host rejects the retry policy or state.
	PolicyError *PolicyError
	// Exhausted reports that the host policy allowed no further retry.
	Exhausted bool
	// Error is the step failure that aborted the transaction, or zero for a panic.
	Error E
	// CompensationFailure is Some when a compensation itself failed during
	// rollback. Check RolledBackFully to include compensation panics.
	CompensationFailure golem.Option[E]
}

// RolledBackFully reports whether every compensation succeeded (a complete
// rollback). When false, some effects may remain and need manual attention.
func (f Failure[E]) RolledBackFully() bool {
	return f.CompensationFailure.IsNone() && f.CompensationPanic == nil
}

// PolicyError is a catchable rejection from the host retry evaluator.
type PolicyError struct{ Message string }

func (e *PolicyError) Error() string { return e.Message }

// Fallible runs f as a transaction. If f returns golem.Ok, the transaction
// commits and its value is returned. If f returns an error, the recorded
// compensations run in reverse (best-effort) and a [Failure] is
// returned — there is no retry. A panic compensates then re-panics with the
// original value after complete rollback. Partial rollback returns Failure.
func Fallible[Out, E any](f func(*Tx[E]) golem.Result[Out, E]) golem.Result[Out, Failure[E]] {
	return runFallible(&Tx[E]{}, f)
}

// runFallible runs the body, rolls back on failure, and assembles the result.
func runFallible[Out, E any](tx *Tx[E], f func(*Tx[E]) golem.Result[Out, E]) golem.Result[Out, Failure[E]] {
	r, p := attempt(tx, f)
	if p != nil || r.IsErr() {
		failure := rollback(tx, r, p)
		if p != nil && failure.RolledBackFully() {
			panic(p)
		}
		return golem.Err[Out, Failure[E]](failure)
	}
	return golem.Ok[Out, Failure[E]](r.Ok())
}

// Infallible retries fully compensated errors and catchable panics according to
// the host's semantic saga policy. Exhaustion, policy errors and partial rollback
// return Failure without rerunning the body. Attempts append durable history.
func Infallible[Out, E any](f func(*Tx[E]) golem.Result[Out, E]) golem.Result[Out, Failure[E]] {
	start := sagaNow()
	var state []byte
	for n := int64(1); ; n++ {
		tx := &Tx[E]{}
		r, p := attempt(tx, f)
		if p == nil && r.IsOk() {
			return golem.Ok[Out, Failure[E]](r.Ok())
		}
		failure := rollback(tx, r, p)
		if !failure.RolledBackFully() {
			return golem.Err[Out, Failure[E]](failure)
		}
		kind := "operation-error"
		if p != nil {
			kind = "panic"
		}
		elapsed := uint64(0)
		if now := sagaNow(); now > start {
			elapsed = now - start
		}
		next, delay, retry, err := sagaNext(state, elapsed, n, kind)
		state = next
		if err != nil || !retry {
			failure.PolicyError = err
			failure.Exhausted = err == nil
			return golem.Err[Out, Failure[E]](failure)
		}
		sagaSleep(delay)
	}
}

var sagaSleep = time.Sleep

func attempt[Out, E any](tx *Tx[E], f func(*Tx[E]) golem.Result[Out, E]) (r golem.Result[Out, E], p any) {
	defer func() { p = recover() }()
	r = f(tx)
	return
}

func rollback[Out, E any](tx *Tx[E], r golem.Result[Out, E], p any) (failure Failure[E]) {
	failure.Panic = p
	if p == nil {
		failure.Error = r.Err()
	}
	defer func() { failure.CompensationPanic = recover() }()
	failure.CompensationFailure = tx.compensate()
	return
}
