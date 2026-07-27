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
	"fmt"
	"reflect"
)

// Go has no sum types, so a WIT variant is expressed as an interface plus the
// set of concrete types that inhabit it. The set has to be declared rather than
// discovered: Go offers no way to enumerate a package's implementors of an
// interface, and the wire format needs a stable case order anyway.
//
//	type PaymentMethod interface{ isPaymentMethod() }
//
//	type Card struct{ Number string }
//	func (Card) isPaymentMethod() {}
//
//	type Cash struct{}
//	func (Cash) isPaymentMethod() {}
//
//	var _ = golem.DefineVariant[PaymentMethod](
//	    golem.Case[Card]("card"),
//	    golem.Case[Cash]("cash"),
//	)
//
// An unexported marker method keeps the set closed: no type outside the
// declaring package can join the variant.

// CaseDef is one case of a variant, produced by [Case].
type CaseDef struct {
	name string
	typ  reflect.Type
}

// Case declares a variant case: the payload type T, carried under name on the
// wire.
func Case[T any](name string) CaseDef {
	return CaseDef{name: name, typ: reflect.TypeFor[T]()}
}

// variantDef is the registered case list for one interface type. Case order is
// declaration order, and it is the wire order — reordering [DefineVariant]'s
// arguments is a breaking change.
type variantDef struct {
	iface reflect.Type
	cases []CaseDef
}

var variantRegistry = map[reflect.Type]*variantDef{}

// DefineVariant registers the closed set of types inhabiting the interface
// Iface. Call it from a package-level var so registration happens before the
// component is invoked.
func DefineVariant[Iface any](cases ...CaseDef) *variantDef {
	it := reflect.TypeFor[Iface]()
	if it.Kind() != reflect.Interface {
		panic(fmt.Sprintf("golem: DefineVariant requires an interface type, got %s", it))
	}
	if len(cases) == 0 {
		panic(fmt.Sprintf("golem: DefineVariant[%s] needs at least one case", it))
	}
	if _, dup := variantRegistry[it]; dup {
		panic(fmt.Sprintf("golem: variant already defined for %s", it))
	}

	seen := map[string]bool{}
	for _, c := range cases {
		if !c.typ.Implements(it) && !reflect.PointerTo(c.typ).Implements(it) {
			panic(fmt.Sprintf("golem: variant case %s (%s) does not implement %s", c.name, c.typ, it))
		}
		if seen[c.name] {
			panic(fmt.Sprintf("golem: variant %s has duplicate case name %q", it, c.name))
		}
		seen[c.name] = true
	}

	d := &variantDef{iface: it, cases: cases}
	variantRegistry[it] = d
	return d
}

// ---------------------------------------------------------------------------
// enums
// ---------------------------------------------------------------------------

// enumDef is the registered name list for one named integer type. A value's
// wire representation is its position in this list, so the Go constants are
// expected to run 0..n-1 in declaration order.
type enumDef struct {
	typ   reflect.Type
	names []string
}

var enumRegistry = map[reflect.Type]*enumDef{}

// DefineEnum registers a named integer type as a WIT enum:
//
//	type Status int32
//	const (
//	    StatusActive Status = iota
//	    StatusClosed
//	)
//
//	var _ = golem.DefineEnum[Status]("active", "closed")
//
// Values are positional: Status(0) is "active". A value outside 0..len(names)-1
// is rejected at encode time rather than silently truncated.
func DefineEnum[T any](names ...string) *enumDef {
	t := reflect.TypeFor[T]()
	switch t.Kind() {
	case reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
		reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
	default:
		panic(fmt.Sprintf("golem: DefineEnum requires a named integer type, got %s (kind %s)", t, t.Kind()))
	}
	if len(names) == 0 {
		panic(fmt.Sprintf("golem: DefineEnum[%s] needs at least one name", t))
	}
	if _, dup := enumRegistry[t]; dup {
		panic(fmt.Sprintf("golem: enum already defined for %s", t))
	}
	d := &enumDef{typ: t, names: names}
	enumRegistry[t] = d
	return d
}
