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

package values

import (
	"errors"
	"reflect"
	"strings"
	"testing"
)

func TestUUIDParsesBothCasesAndRendersLowercase(t *testing.T) {
	const lower = "dd00721b-3329-4621-a01d-c71f02cd78c6"
	for _, in := range []string{lower, strings.ToUpper(lower)} {
		u, err := ParseUUID(in)
		if err != nil {
			t.Fatalf("%s: %v", in, err)
		}
		if u.String() != lower {
			t.Errorf("%s rendered as %s", in, u)
		}
	}
	for _, bad := range []string{
		"",
		"dd00721b33294621a01dc71f02cd78c6",
		"dd00721b-3329-4621-a01d-c71f02cd78c",
		"dd00721b-3329-4621-a01d-c71f02cd78c6a",
		"dd00721b_3329-4621-a01d-c71f02cd78c6",
		"gd00721b-3329-4621-a01d-c71f02cd78c6",
		"{dd00721b-3329-4621-a01d-c71f02cd78c6}",
	} {
		if _, err := ParseUUID(bad); err == nil {
			t.Errorf("accepted %q", bad)
		}
	}
	var zero UUID
	if zero.String() != "00000000-0000-0000-0000-000000000000" {
		t.Errorf("zero UUID rendered as %s", zero)
	}
}

func TestOptionAccessors(t *testing.T) {
	some, none := Some(7), None[int]()
	if v, ok := some.Get(); !ok || v != 7 || !some.IsSome() || some.IsNone() {
		t.Errorf("Some(7) = %v %v", v, ok)
	}
	if _, ok := none.Get(); ok || none.IsSome() || !none.IsNone() {
		t.Error("None reports a value")
	}
	if some.Or(1) != 7 || none.Or(1) != 1 {
		t.Error("Or ignores presence")
	}
	if some.Unwrap() != 7 {
		t.Error("Unwrap lost the value")
	}
	defer func() {
		if recover() == nil {
			t.Error("Unwrap on None did not panic")
		}
	}()
	none.Unwrap()
}

type codeErr struct{ Code int }

func TestResultBridgesToGoErrors(t *testing.T) {
	ok := Ok[int, string](3)
	failed := Err[int, string]("boom")
	if v, err := ok.Get(); err != nil || v != 3 || !ok.IsOk() || ok.IsErr() {
		t.Errorf("Ok(3).Get() = %v, %v", v, err)
	}
	_, err := failed.Get()
	var re *ResultError[string]
	if !errors.As(err, &re) || re.Value != "boom" || err.Error() != "boom" {
		t.Errorf("Err(boom).Get() = %v", err)
	}
	if failed.OkOr(9) != 9 || ok.OkOr(9) != 3 {
		t.Error("OkOr ignores the arm")
	}
	if failed.Err() != "boom" || ok.Ok() != 3 {
		t.Error("typed arms lost their values")
	}
	sentinel := errors.New("already an error")
	if _, err := Err[int, error](sentinel).Get(); err != sentinel {
		t.Errorf("an error Err arm was wrapped: %v", err)
	}
	if _, err := Err[int, codeErr](codeErr{Code: 4}).Get(); err == nil {
		t.Error("a struct Err arm was dropped")
	}
	for name, f := range map[string]func(){
		"Ok on a failure":   func() { failed.Ok() },
		"Err on a success":  func() { ok.Err() },
		"MustOk on failure": func() { failed.MustOk() },
	} {
		func() {
			defer func() {
				if recover() == nil {
					t.Errorf("%s did not panic", name)
				}
			}()
			f()
		}()
	}
}

type kilograms struct{}

func (kilograms) BaseUnit() string          { return "kg" }
func (kilograms) AllowedSuffixes() []string { return []string{"kg", "g"} }

func TestQuantityPlumbing(t *testing.T) {
	q := Quantity[kilograms]{Mantissa: 1500, Scale: 3}
	if unit, ok := QuantityUnitOf(q); !ok || unit.BaseUnit() != "kg" {
		t.Errorf("unit = %v %v", unit, ok)
	}
	// An empty unit is the base unit.
	if m, s, u, ok := QuantityParts(q); !ok || m != 1500 || s != 3 || u != "kg" {
		t.Errorf("parts = %d %d %q %v", m, s, u, ok)
	}
	var back Quantity[kilograms]
	if !QuantitySetParts(&back, 2, 0, "g") || back != (Quantity[kilograms]{Mantissa: 2, Unit: "g"}) {
		t.Errorf("set parts = %+v", back)
	}
	if _, _, _, ok := QuantityParts(42); ok {
		t.Error("an int reported quantity parts")
	}
}

func TestOptionAndResultPlumbing(t *testing.T) {
	if elem, ok := OptionElem(Some("x")); !ok || elem != reflect.TypeFor[string]() {
		t.Errorf("option elem = %v %v", elem, ok)
	}
	var o Option[int]
	v, ok := OptionSetSome(&o)
	if !ok {
		t.Fatal("OptionSetSome refused an *Option")
	}
	v.SetInt(5)
	if got, some := o.Get(); !some || got != 5 {
		t.Errorf("set some = %v %v", got, some)
	}
	if !OptionSetNone(&o) || o.IsSome() {
		t.Error("OptionSetNone left a value")
	}
	if _, _, ok := OptionGet(7); ok {
		t.Error("an int read as an Option")
	}

	var r Result[int, string]
	e, ok := ResultSetErr(&r)
	if !ok {
		t.Fatal("ResultSetErr refused a *Result")
	}
	e.SetString("bad")
	if elem, isErr, ok := ResultGet(r); !ok || !isErr || elem.String() != "bad" {
		t.Errorf("result get = %v %v %v", elem, isErr, ok)
	}
	if okT, errT, ok := ResultElems(r); !ok || okT != reflect.TypeFor[int]() || errT != reflect.TypeFor[string]() {
		t.Errorf("result elems = %v %v %v", okT, errT, ok)
	}
}

func TestTupleElemsInDeclarationOrder(t *testing.T) {
	elems, ok := TupleElems(Tuple3[string, int, bool]{})
	want := []reflect.Type{reflect.TypeFor[string](), reflect.TypeFor[int](), reflect.TypeFor[bool]()}
	if !ok || !reflect.DeepEqual(elems, want) {
		t.Errorf("elems = %v %v", elems, ok)
	}
	if _, ok := TupleElems(struct{ A, B int }{}); ok {
		t.Error("a plain struct read as a tuple")
	}
}
