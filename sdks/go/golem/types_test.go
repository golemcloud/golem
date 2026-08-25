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
	"errors"
	"testing"
)

func TestOptionHelpers(t *testing.T) {
	s := Some(7)
	if !s.IsSome() || s.IsNone() {
		t.Fatal("Some should be present")
	}
	if v, ok := s.Get(); !ok || v != 7 {
		t.Fatalf("Get = %d,%v", v, ok)
	}
	if s.Or(9) != 7 || s.Unwrap() != 7 {
		t.Fatal("Or/Unwrap on present")
	}

	n := None[int]()
	if n.IsSome() || !n.IsNone() {
		t.Fatal("None should be empty")
	}
	if _, ok := n.Get(); ok {
		t.Fatal("None Get reports present")
	}
	if n.Or(9) != 9 {
		t.Fatal("Or should return the default")
	}
	mustPanic(t, "empty Option", func() { n.Unwrap() })
}

func TestResultHelpers(t *testing.T) {
	ok := Ok[int, string](3)
	if !ok.IsOk() || ok.IsErr() || ok.Ok() != 3 || ok.OkOr(9) != 3 || ok.MustOk() != 3 {
		t.Fatal("Ok result")
	}
	if v, err := ok.Get(); err != nil || v != 3 {
		t.Fatalf("Get = %d,%v", v, err)
	}

	er := Err[int, string]("boom")
	if er.IsOk() || !er.IsErr() || er.Err() != "boom" || er.OkOr(9) != 9 {
		t.Fatal("Err result")
	}

	mustPanic(t, "Result.Ok() on a failed Result", func() { er.Ok() })
	mustPanic(t, "Result.Err() on a successful Result", func() { ok.Err() })
	mustPanic(t, "boom", func() { er.MustOk() })
}

// TestMustOkPanicsWithTypedError — MustOk panics with the same error Get() would
// return, so a recover() can pull the typed Err payload back out via errors.As.
func TestMustOkPanicsWithTypedError(t *testing.T) {
	defer func() {
		r := recover()
		err, isErr := r.(error)
		if !isErr {
			t.Fatalf("MustOk should panic with an error, got %#v", r)
		}
		var re *ResultError[string]
		if !errors.As(err, &re) || re.Value != "boom" {
			t.Fatalf("panic did not carry the typed payload: %v", err)
		}
	}()
	Err[int, string]("boom").MustOk()
	t.Fatal("MustOk on a failed Result should have panicked")
}

// TestResultGetBridgesToError — Get returns (value, error): nil error on Ok, and
// on Err a ResultError carrying the typed payload (recoverable via errors.As).
func TestResultGetBridgesToError(t *testing.T) {
	if v, err := Ok[int, string](3).Get(); err != nil || v != 3 {
		t.Fatalf("Ok.Get() = %d, %v", v, err)
	}

	_, err := Err[int, string]("boom").Get()
	if err == nil || err.Error() != "boom" {
		t.Fatalf("Err.Get() error = %v", err)
	}
	var re *ResultError[string]
	if !errors.As(err, &re) || re.Value != "boom" {
		t.Fatalf("errors.As did not recover the typed payload: %v", err)
	}

	// An Err arm that already implements error is passed through unwrapped.
	sentinel := errors.New("native")
	_, err = Err[int, error](sentinel).Get()
	if !errors.Is(err, sentinel) {
		t.Fatalf("native-error Err arm should pass through: %v", err)
	}
}

func TestBridges(t *testing.T) {
	if Must(7, nil) != 7 {
		t.Fatal("Must should return the value")
	}
	mustPanic(t, "bang", func() { Must(0, errors.New("bang")) })

	if r := ResultOf(5, nil); !r.IsOk() || r.Ok() != 5 {
		t.Fatal("ResultOf(nil) should be Ok")
	}
	if r := ResultOf(0, errors.New("nope")); !r.IsErr() || r.Err() != "nope" {
		t.Fatal("ResultOf(err) should carry the message")
	}

	a, b := Must2("a", 2, nil)
	if a != "a" || b != 2 {
		t.Fatalf("Must2 = %q,%d", a, b)
	}
	mustPanic(t, "bad", func() { Must2("", 0, errors.New("bad")) })

	Must0(nil) // no panic on success
	mustPanic(t, "boom0", func() { Must0(errors.New("boom0")) })
}
