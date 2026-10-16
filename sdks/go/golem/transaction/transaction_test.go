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
	"reflect"
	"testing"
	"time"

	"github.com/golemcloud/golem/sdks/go/golem"
)

func harness(t *testing.T) (*uint64, *[]time.Duration) {
	t.Helper()
	now, waits := uint64(100), []time.Duration{}
	oldNow, oldNext, oldSleep := sagaNow, sagaNext, sagaSleep
	t.Cleanup(func() { sagaNow, sagaNext, sagaSleep = oldNow, oldNext, oldSleep })
	sagaNow = func() uint64 { return now }
	sagaSleep = func(d time.Duration) { waits = append(waits, d); now += uint64(d) }
	return &now, &waits
}

func step(tx *Tx[string], calls *[]string, name string, compensate func() golem.Result[golem.Unit, string]) {
	tx.Step(NewOperation(
		func(in int) golem.Result[int, string] {
			*calls = append(*calls, "e"+name)
			return golem.Ok[int, string](in)
		},
		func(_, _ int) golem.Result[golem.Unit, string] {
			*calls = append(*calls, "c"+name)
			return compensate()
		},
	), 1)
}

func ok() golem.Result[golem.Unit, string] { return golem.Ok[golem.Unit, string](golem.Unit{}) }

func TestFallibleCommitAndRollback(t *testing.T) {
	for _, fail := range []bool{false, true} {
		var calls []string
		r := Fallible(func(tx *Tx[string]) golem.Result[int, string] {
			step(tx, &calls, "1", ok)
			step(tx, &calls, "2", ok)
			if fail {
				return golem.Err[int, string]("body")
			}
			return golem.Ok[int, string](42)
		})
		want := []string{"e1", "e2"}
		if fail {
			want = append(want, "c2", "c1")
			if r.IsOk() || r.Err().Error != "body" || !r.Err().RolledBackFully() {
				t.Fatalf("failure: %+v", r)
			}
		} else if r.IsErr() || r.Ok() != 42 {
			t.Fatalf("commit: %+v", r)
		}
		if !reflect.DeepEqual(calls, want) {
			t.Fatalf("calls=%v want=%v", calls, want)
		}
	}
}

func TestFalliblePanicCompensatesAndPreservesIdentity(t *testing.T) {
	original := &struct{ message string }{"original"}
	var calls []string
	defer func() {
		if p := recover(); p != original {
			t.Fatalf("panic=%v", p)
		}
		if !reflect.DeepEqual(calls, []string{"e1", "e2", "c2", "c1"}) {
			t.Fatalf("calls=%v", calls)
		}
	}()
	Fallible(func(tx *Tx[string]) golem.Result[int, string] {
		step(tx, &calls, "1", ok)
		step(tx, &calls, "2", ok)
		panic(original)
	})
}

func TestFalliblePanicPartialRollbackPreservesEvidence(t *testing.T) {
	for _, panics := range []bool{false, true} {
		original := &struct{ message string }{"body"}
		rollbackError := &struct{ message string }{"rollback"}
		r := Fallible(func(tx *Tx[*struct{ message string }]) golem.Result[int, *struct{ message string }] {
			tx.Step(NewOperation(
				func(in int) golem.Result[int, *struct{ message string }] {
					return golem.Ok[int, *struct{ message string }](in)
				},
				func(_, _ int) golem.Result[golem.Unit, *struct{ message string }] {
					if panics {
						panic(rollbackError)
					}
					return golem.Err[golem.Unit, *struct{ message string }](rollbackError)
				}), 1)
			panic(original)
		})
		if !r.IsErr() {
			t.Fatal("partial rollback succeeded")
		}
		failure := r.Err()
		if failure.Panic != original || failure.RolledBackFully() {
			t.Fatalf("lost body panic: %+v", failure)
		}
		if panics {
			if failure.CompensationPanic != rollbackError {
				t.Fatal("lost compensation panic identity")
			}
		} else if failure.CompensationFailure.IsNone() || failure.CompensationFailure.Unwrap() != rollbackError {
			t.Fatal("lost compensation error identity")
		}
	}
}

func TestInfallibleRetriesWithStateElapsedAndWait(t *testing.T) {
	now, waits := harness(t)
	for _, panics := range []bool{false, true} {
		*now = 100
		*waits = nil
		attempts, decisions := 0, 0
		var calls []string
		sagaNext = func(state []byte, elapsed uint64, attempt int64, kind string) ([]byte, time.Duration, bool, *PolicyError) {
			decisions++
			wantKind := "operation-error"
			if panics {
				wantKind = "panic"
			}
			if kind != wantKind || attempt != int64(decisions) || elapsed != uint64(decisions*7+(decisions-1)*10) {
				t.Fatalf("context: %d %d %s", elapsed, attempt, kind)
			}
			if decisions == 1 && len(state) != 0 || decisions == 2 && string(state) != "token" {
				t.Fatalf("state=%q", state)
			}
			return []byte("token"), 10, true, nil
		}
		r := Infallible(func(tx *Tx[string]) golem.Result[int, string] {
			attempts++
			*now += 3
			step(tx, &calls, "1", func() golem.Result[golem.Unit, string] { *now += 4; return ok() })
			if attempts == 3 {
				return golem.Ok[int, string](42)
			}
			if panics {
				panic("body")
			}
			return golem.Err[int, string]("body")
		})
		if r.IsErr() || r.Ok() != 42 || attempts != 3 || decisions != 2 || !reflect.DeepEqual(*waits, []time.Duration{10, 10}) {
			t.Fatalf("result=%+v attempts=%d waits=%v", r, attempts, *waits)
		}
		if !reflect.DeepEqual(calls, []string{"e1", "c1", "e1", "c1", "e1"}) {
			t.Fatalf("calls=%v", calls)
		}
	}
}

func TestInfallibleClampsBackwardClock(t *testing.T) {
	for _, readings := range [][]uint64{{90}, {120, 90}} {
		t.Run(fmt.Sprint(readings), func(t *testing.T) {
			now, waits := harness(t)
			attempts, decisions := 0, 0
			sagaNext = func(state []byte, elapsed uint64, attempt int64, kind string) ([]byte, time.Duration, bool, *PolicyError) {
				want := uint64(0)
				if readings[decisions] > 100 {
					want = readings[decisions] - 100
				}
				if elapsed != want || attempt != int64(decisions+1) || len(state) != decisions || kind != "operation-error" {
					t.Fatalf("context: elapsed=%d want=%d attempt=%d state=%v kind=%s", elapsed, want, attempt, state, kind)
				}
				decisions++
				return append(state, 1), 10, elapsed < 50, nil
			}
			r := Infallible(func(*Tx[string]) golem.Result[int, string] {
				attempts++
				if attempts > len(readings) {
					return golem.Ok[int, string](42)
				}
				*now = readings[attempts-1]
				return golem.Err[int, string]("retry")
			})
			if r.IsErr() || r.Ok() != 42 || decisions != len(readings) || len(*waits) != len(readings) {
				t.Fatalf("result=%+v decisions=%d waits=%v", r, decisions, *waits)
			}
		})
	}
}

func TestInfallibleTypedFailuresPreserveOriginal(t *testing.T) {
	for _, mode := range []string{"exhausted", "policy", "partial-error", "partial-panic"} {
		for _, panics := range []bool{false, true} {
			t.Run(mode+map[bool]string{true: "/panic", false: "/error"}[panics], func(t *testing.T) {
				harness(t)
				original := &struct{ message string }{"original"}
				rollbackPanic := &struct{ message string }{"rollback"}
				policy := &PolicyError{Message: "changed policy"}
				decisions, attempts := 0, 0
				sagaNext = func([]byte, uint64, int64, string) ([]byte, time.Duration, bool, *PolicyError) {
					decisions++
					if mode == "policy" {
						return nil, 0, false, policy
					}
					return nil, 0, false, nil
				}
				var calls []string
				r := Infallible(func(tx *Tx[string]) golem.Result[int, string] {
					attempts++
					step(tx, &calls, "older", ok)
					step(tx, &calls, "newer", func() golem.Result[golem.Unit, string] {
						if mode == "partial-panic" {
							panic(rollbackPanic)
						}
						if mode == "partial-error" {
							return golem.Err[golem.Unit, string]("rollback")
						}
						return ok()
					})
					if panics {
						panic(original)
					}
					return golem.Err[int, string]("original")
				})
				if r.IsOk() || attempts != 1 {
					t.Fatalf("result=%+v attempts=%d", r, attempts)
				}
				f := r.Err()
				if panics && f.Panic != original || !panics && f.Error != "original" {
					t.Fatalf("original lost: %+v", f)
				}
				switch mode {
				case "exhausted":
					if !f.Exhausted || decisions != 1 {
						t.Fatalf("%+v", f)
					}
				case "policy":
					if f.PolicyError != policy || f.Exhausted {
						t.Fatalf("%+v", f)
					}
				case "partial-error", "partial-panic":
					if f.RolledBackFully() || decisions != 0 || !reflect.DeepEqual(calls, []string{"eolder", "enewer", "cnewer"}) {
						t.Fatalf("failure=%+v decisions=%d calls=%v", f, decisions, calls)
					}
					if mode == "partial-panic" && f.CompensationPanic != rollbackPanic {
						t.Fatalf("rollback identity lost: %+v", f)
					}
					if mode == "partial-error" && f.CompensationFailure.Unwrap() != "rollback" {
						t.Fatalf("rollback error lost: %+v", f)
					}
				}
			})
		}
	}
}

func TestPanickingStepDoesNotRegisterCompensation(t *testing.T) {
	harness(t)
	original := &struct{ message string }{"step panic"}
	sagaNext = func([]byte, uint64, int64, string) ([]byte, time.Duration, bool, *PolicyError) {
		return nil, 0, false, nil
	}
	var calls []string
	r := Infallible(func(tx *Tx[string]) golem.Result[int, string] {
		step(tx, &calls, "older", ok)
		return tx.Step(NewOperation(
			func(int) golem.Result[int, string] { panic(original) },
			func(int, int) golem.Result[golem.Unit, string] {
				t.Fatal("panicking step registered compensation")
				return ok()
			},
		), 1)
	})
	if r.IsOk() || r.Err().Panic != original || !r.Err().Exhausted || !reflect.DeepEqual(calls, []string{"eolder", "colder"}) {
		t.Fatalf("result=%+v calls=%v", r, calls)
	}
}

func TestInfallibleBoundedExhaustion(t *testing.T) {
	_, waits := harness(t)
	attempts := 0
	sagaNext = func(state []byte, _ uint64, attempt int64, _ string) ([]byte, time.Duration, bool, *PolicyError) {
		if len(state) != int(attempt)-1 {
			t.Fatalf("state=%v attempt=%d", state, attempt)
		}
		return append(state, 1), time.Millisecond, attempt < 3, nil
	}
	r := Infallible(func(*Tx[string]) golem.Result[int, string] { attempts++; return golem.Err[int, string]("original") })
	if r.IsOk() || !r.Err().Exhausted || r.Err().Error != "original" || attempts != 3 || len(*waits) != 2 {
		t.Fatalf("result=%+v attempts=%d waits=%v", r, attempts, *waits)
	}
}

func TestCompensationUsesAttemptInputsAndOutputs(t *testing.T) {
	harness(t)
	sagaNext = func([]byte, uint64, int64, string) ([]byte, time.Duration, bool, *PolicyError) {
		return nil, 0, true, nil
	}
	attempts, identity := 0, 0
	var compensated [][2]int
	op := NewOperation(
		func(int) golem.Result[int, string] { identity++; return golem.Ok[int, string](identity) },
		func(in, out int) golem.Result[golem.Unit, string] {
			compensated = append(compensated, [2]int{in, out})
			return ok()
		},
	)
	r := Infallible(func(tx *Tx[string]) golem.Result[int, string] {
		attempts++
		tx.Step(op, 10)
		last := tx.Step(op, 70)
		if attempts < 3 {
			return golem.Err[int, string]("retry")
		}
		return last
	})
	if r.IsErr() || r.Ok() != 6 || !reflect.DeepEqual(compensated, [][2]int{{70, 2}, {10, 1}, {70, 4}, {10, 3}}) {
		t.Fatalf("result=%+v compensated=%v", r, compensated)
	}
}
