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

// DefineVariant registers the closed set of types inhabiting the interface
// Iface. Call it from a package-level var so registration happens before the
// component is invoked.
func DefineVariant[Iface any](cases ...CaseDef) *variantDef {
	return defineVariantInto[Iface](defs, cases...)
}

// defineVariantInto is the instance-scoped implementation behind DefineVariant.
func defineVariantInto[Iface any](d *definitions, cases ...CaseDef) *variantDef {
	it := reflect.TypeFor[Iface]()
	vd := &variantDef{iface: it, cases: cases}
	if it.Kind() != reflect.Interface {
		d.recordErr("", "", "DefineVariant requires an interface type, got %s", it)
		return vd
	}
	if _, dup := d.variants[it]; dup {
		d.recordErr("", "", "variant already defined for %s", it)
		return vd
	}
	if len(cases) == 0 {
		d.recordErr("", "", "DefineVariant[%s] needs at least one case", it)
	}

	seen := map[string]bool{}
	seenType := map[reflect.Type]bool{}
	for _, c := range cases {
		if !c.typ.Implements(it) && !reflect.PointerTo(c.typ).Implements(it) {
			d.recordErr("", "", "variant case %s (%s) does not implement %s", c.name, c.typ, it)
		}
		if seen[c.name] {
			d.recordErr("", "", "variant %s has duplicate case name %q", it, c.name)
		}
		if seenType[c.typ] {
			// The wire case is chosen by the value's dynamic type, so one type
			// cannot map to two case names unambiguously.
			d.recordErr("", "", "variant %s uses case type %s more than once", it, c.typ)
		}
		seen[c.name] = true
		seenType[c.typ] = true
	}

	// Registered even with soft errors above, so downstream codec compilation
	// resolves the interface as a variant instead of cascading a second error.
	d.variants[it] = vd
	return vd
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
	return defineEnumInto[T](defs, names...)
}

// defineEnumInto is the instance-scoped implementation behind DefineEnum.
func defineEnumInto[T any](d *definitions, names ...string) *enumDef {
	t := reflect.TypeFor[T]()
	ed := &enumDef{typ: t, names: names}
	switch t.Kind() {
	case reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
		reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
	default:
		d.recordErr("", "", "DefineEnum requires a named integer type, got %s (kind %s)", t, t.Kind())
		return ed
	}
	if _, dup := d.enums[t]; dup {
		d.recordErr("", "", "enum already defined for %s", t)
		return ed
	}
	if len(names) == 0 {
		d.recordErr("", "", "DefineEnum[%s] needs at least one name", t)
	}
	d.enums[t] = ed
	return ed
}
