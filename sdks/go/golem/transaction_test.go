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
	"testing"
)

// tracker records execute/compensate calls in order, so tests can assert that
// compensations run in reverse.
type tracker struct{ calls []string }

// okOp is an operation that succeeds (recording "e<tag>") and whose compensation
// succeeds (recording "c<tag>").
func okOp(tr *tracker, tag string) Operation[int, int, string] {
	return NewOperation(
		func(in int) Result[int, string] {
			tr.calls = append(tr.calls, "e"+tag)
			return Ok[int, string](in + 1)
		},
		func(in, out int) Result[Unit, string] {
			tr.calls = append(tr.calls, "c"+tag)
			return Ok[Unit, string](Unit{})
		},
	)
}

// failOp is an operation whose execute fails, recording "e<tag>".
func failOp(tr *tracker, tag, reason string) Operation[int, int, string] {
	return NewOperation(
		func(in int) Result[int, string] {
			tr.calls = append(tr.calls, "e"+tag)
			return Err[int, string](reason)
		},
		func(in, out int) Result[Unit, string] {
			tr.calls = append(tr.calls, "c"+tag)
			return Ok[Unit, string](Unit{})
		},
	)
}

func eq(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// TestFallibleCommit — every step succeeds: no compensation runs and the value is
// returned.
func TestFallibleCommit(t *testing.T) {
	tr := &tracker{}
	a, b := okOp(tr, "1"), okOp(tr, "2")
	out := runFallible(&Transaction[string]{}, func(tx *Transaction[string]) Result[int, string] {
		r1 := Step(tx, a, 10)
		r2 := Step(tx, b, r1.Ok())
		return Ok[int, string](r2.Ok())
	})
	if out.IsErr() {
		t.Fatalf("expected commit, got %+v", out.Err())
	}
	if out.Ok() != 12 {
		t.Fatalf("value = %d, want 12", out.Ok())
	}
	if !eq(tr.calls, []string{"e1", "e2"}) {
		t.Fatalf("calls = %v, want [e1 e2] (no compensation)", tr.calls)
	}
}

// TestFallibleRollbackComplete — a mid-transaction failure compensates the
// already-succeeded steps in REVERSE order, and reports a complete rollback.
func TestFallibleRollbackComplete(t *testing.T) {
	tr := &tracker{}
	a, b := okOp(tr, "1"), okOp(tr, "2")
	bad := failOp(tr, "3", "boom")
	out := runFallible(&Transaction[string]{}, func(tx *Transaction[string]) Result[int, string] {
		if r := Step(tx, a, 1); r.IsErr() {
			return r
		}
		if r := Step(tx, b, 2); r.IsErr() {
			return r
		}
		if r := Step(tx, bad, 3); r.IsErr() {
			return r
		}
		return Ok[int, string](0)
	})
	if !out.IsErr() {
		t.Fatal("expected failure")
	}
	f := out.Err()
	if f.Error != "boom" {
		t.Fatalf("error = %q, want boom", f.Error)
	}
	if !f.RolledBackFully() {
		t.Fatalf("expected complete rollback, got partial: %+v", f.CompensationFailure)
	}
	// e1,e2 succeeded and recorded compensations; e3 failed (no compensation).
	// Rollback runs c2 then c1 (reverse).
	if !eq(tr.calls, []string{"e1", "e2", "e3", "c2", "c1"}) {
		t.Fatalf("calls = %v, want [e1 e2 e3 c2 c1]", tr.calls)
	}
}

// TestFallibleRollbackPartial — a compensation that itself fails is reported as a
// partial rollback carrying the compensation's error.
func TestFallibleRollbackPartial(t *testing.T) {
	tr := &tracker{}
	// a's compensation fails; b succeeds; c fails execution to trigger rollback.
	a := NewOperation(
		func(in int) Result[int, string] { tr.calls = append(tr.calls, "ea"); return Ok[int, string](in) },
		func(in, out int) Result[Unit, string] {
			tr.calls = append(tr.calls, "ca")
			return Err[Unit, string]("ca-failed")
		},
	)
	b := okOp(tr, "b")
	bad := failOp(tr, "c", "boom")
	out := runFallible(&Transaction[string]{}, func(tx *Transaction[string]) Result[int, string] {
		if r := Step(tx, a, 1); r.IsErr() {
			return r
		}
		if r := Step(tx, b, 2); r.IsErr() {
			return r
		}
		if r := Step(tx, bad, 3); r.IsErr() {
			return r
		}
		return Ok[int, string](0)
	})
	f := out.Err()
	if f.Error != "boom" {
		t.Fatalf("error = %q, want boom", f.Error)
	}
	if f.RolledBackFully() {
		t.Fatal("expected partial rollback")
	}
	if cf, _ := f.CompensationFailure.Get(); cf != "ca-failed" {
		t.Fatalf("compensation failure = %q, want ca-failed", cf)
	}
	// Rollback runs cb (ok) then ca (fails).
	if !eq(tr.calls, []string{"ea", "eb", "ec", "cb", "ca"}) {
		t.Fatalf("calls = %v, want [ea eb ec cb ca]", tr.calls)
	}
}
