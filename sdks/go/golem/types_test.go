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
	if !ok.IsOk() || ok.IsErr() || ok.Ok() != 3 || ok.OkOr(9) != 3 {
		t.Fatal("Ok result")
	}
	if v, good := ok.Get(); !good || v != 3 {
		t.Fatalf("Get = %d,%v", v, good)
	}

	er := Err[int, string]("boom")
	if er.IsOk() || !er.IsErr() || er.Err() != "boom" || er.OkOr(9) != 9 {
		t.Fatal("Err result")
	}

	mustPanic(t, "Ok on a failed Result", func() { er.Ok() })
	mustPanic(t, "Err on a successful Result", func() { ok.Err() })
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
}
