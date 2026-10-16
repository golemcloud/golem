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
	"strings"
	"testing"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
)

// tracker records execute/compensate calls in order, so tests can assert that
// compensations run in reverse.
type tracker struct{ calls []string }

// okOp is an operation that succeeds (recording "e<tag>") and whose compensation
// succeeds (recording "c<tag>").
func okOp(tr *tracker, tag string) Operation[int, int, string] {
	return NewOperation(
		func(in int) golem.Result[int, string] {
			tr.calls = append(tr.calls, "e"+tag)
			return golem.Ok[int, string](in + 1)
		},
		func(in, out int) golem.Result[golem.Unit, string] {
			tr.calls = append(tr.calls, "c"+tag)
			return golem.Ok[golem.Unit, string](golem.Unit{})
		},
	)
}

// failOp is an operation whose execute fails, recording "e<tag>".
func failOp(tr *tracker, tag, reason string) Operation[int, int, string] {
	return NewOperation(
		func(in int) golem.Result[int, string] {
			tr.calls = append(tr.calls, "e"+tag)
			return golem.Err[int, string](reason)
		},
		func(in, out int) golem.Result[golem.Unit, string] {
			tr.calls = append(tr.calls, "c"+tag)
			return golem.Ok[golem.Unit, string](golem.Unit{})
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
	out := Fallible(func(tx *Tx[string]) golem.Result[int, string] {
		r1 := tx.Step(a, 10)
		r2 := tx.Step(b, r1.Ok())
		return golem.Ok[int, string](r2.Ok())
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
	out := Fallible(func(tx *Tx[string]) golem.Result[int, string] {
		if r := tx.Step(a, 1); r.IsErr() {
			return r
		}
		if r := tx.Step(b, 2); r.IsErr() {
			return r
		}
		if r := tx.Step(bad, 3); r.IsErr() {
			return r
		}
		return golem.Ok[int, string](0)
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
		func(in int) golem.Result[int, string] {
			tr.calls = append(tr.calls, "ea")
			return golem.Ok[int, string](in)
		},
		func(in, out int) golem.Result[golem.Unit, string] {
			tr.calls = append(tr.calls, "ca")
			return golem.Err[golem.Unit, string]("ca-failed")
		},
	)
	b := okOp(tr, "b")
	bad := failOp(tr, "c", "boom")
	out := Fallible(func(tx *Tx[string]) golem.Result[int, string] {
		if r := tx.Step(a, 1); r.IsErr() {
			return r
		}
		if r := tx.Step(b, 2); r.IsErr() {
			return r
		}
		if r := tx.Step(bad, 3); r.IsErr() {
			return r
		}
		return golem.Ok[int, string](0)
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

func TestInfallibleRetriesWithFreshCompensations(t *testing.T) {
	tr := &tracker{}
	identity := 0
	reserve := NewOperation(
		func(in int) golem.Result[int, string] {
			identity++
			tr.calls = append(tr.calls, fmt.Sprintf("reserve:%d:%d", in, identity))
			return golem.Ok[int, string](identity)
		},
		func(in, out int) golem.Result[golem.Unit, string] {
			tr.calls = append(tr.calls, fmt.Sprintf("release:%d:%d", in, out))
			return golem.Ok[golem.Unit, string](golem.Unit{})
		},
	)
	attempts := 0
	settle := NewOperation(
		func(in int) golem.Result[int, string] {
			tr.calls = append(tr.calls, fmt.Sprintf("settle:%d", in))
			if attempts < 3 {
				return golem.Err[int, string]("declined")
			}
			return golem.Ok[int, string](in)
		},
		func(in, out int) golem.Result[golem.Unit, string] {
			t.Fatal("failed or committed settlement must not be compensated")
			return golem.Ok[golem.Unit, string](golem.Unit{})
		},
	)
	out := Infallible(func(tx *Tx[string]) golem.Result[int, string] {
		attempts++
		a := tx.Step(reserve, 10).Ok()
		b := tx.Step(reserve, 70).Ok()
		return tx.Step(settle, 100*a+b)
	})
	if out != 506 || attempts != 3 {
		t.Fatalf("result=%d attempts=%d", out, attempts)
	}
	if !eq(tr.calls, []string{
		"reserve:10:1", "reserve:70:2", "settle:102", "release:70:2", "release:10:1",
		"reserve:10:3", "reserve:70:4", "settle:304", "release:70:4", "release:10:3",
		"reserve:10:5", "reserve:70:6", "settle:506",
	}) {
		t.Fatalf("calls = %v", tr.calls)
	}
}

func TestInfallibleStopsOnCompensationFailure(t *testing.T) {
	for _, mode := range []string{"error", "panic"} {
		t.Run(mode, func(t *testing.T) {
			attempts := 0
			tr := &tracker{}
			defer func() {
				failure := recover()
				if !strings.Contains(fmt.Sprint(failure), "refund rejected") || attempts != 1 {
					t.Fatalf("expected compensation panic without retry, failure=%v attempts=%d", failure, attempts)
				}
				if !eq(tr.calls, []string{"eolder", "refund"}) {
					t.Fatalf("compensation must stop at failure: %v", tr.calls)
				}
			}()
			op := NewOperation(
				func(in int) golem.Result[int, string] { return golem.Ok[int, string](in) },
				func(in, out int) golem.Result[golem.Unit, string] {
					tr.calls = append(tr.calls, "refund")
					if mode == "panic" {
						panic("refund rejected")
					}
					return golem.Err[golem.Unit, string]("refund rejected")
				},
			)
			Infallible(func(tx *Tx[string]) golem.Result[int, string] {
				attempts++
				if attempts > 1 {
					return golem.Ok[int, string](0)
				}
				tx.Step(okOp(tr, "older"), 70)
				tx.Step(op, 1)
				return golem.Err[int, string]("retry")
			})
		})
	}
}

func TestUnexpectedPanicsTrapWithoutCompensationOrRetry(t *testing.T) {
	for _, mode := range []string{"fallible", "infallible"} {
		for _, location := range []string{"body", "step"} {
			t.Run(mode+"/"+location, func(t *testing.T) {
				previous := link.TrapPanic
				t.Cleanup(func() { link.TrapPanic = previous })
				original := &struct{ Reason string }{"unexpected failure"}
				trapped := &struct{ Reason string }{"host trap"}
				traps := 0
				link.TrapPanic = func(region string, value any) {
					traps++
					if region != "transaction" || value != original {
						t.Fatalf("wrong trap: region=%q value=%v", region, value)
					}
					panic(trapped)
				}
				tr := &tracker{}
				attempts := 0
				defer func() {
					if failure := recover(); failure != trapped {
						t.Fatalf("expected host trap, got %v", failure)
					}
					if attempts != 1 || traps != 1 || !eq(tr.calls, []string{"eflight", "ehotel"}) {
						t.Fatalf("attempts=%d traps=%d calls=%v", attempts, traps, tr.calls)
					}
				}()
				body := func(tx *Tx[string]) golem.Result[int, string] {
					attempts++
					if attempts > 1 {
						return golem.Ok[int, string](0)
					}
					tx.Step(okOp(tr, "flight"), 17)
					tx.Step(okOp(tr, "hotel"), 83)
					if location == "body" {
						panic(original)
					}
					return tx.Step(NewOperation(
						func(in int) golem.Result[int, string] { panic(original) },
						func(in, out int) golem.Result[golem.Unit, string] {
							t.Fatal("a panicking step must not register compensation")
							return golem.Ok[golem.Unit, string](golem.Unit{})
						},
					), 91)
				}
				if mode == "fallible" {
					Fallible(body)
				} else {
					Infallible(body)
				}
			})
		}
	}
}
