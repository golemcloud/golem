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

	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
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
// A case type is its payload: Card's fields are what the card case carries. A
// case with nothing to carry is an empty struct, and publishes as a case with
// no payload — Cash above is `cash`, not `cash(record {})`. That matters beyond
// Go: it is the only way to match a payloadless case another language declares.
//
// An unexported marker method keeps the set closed: no type outside the
// declaring package can join the variant.

// CaseDef is one case of a variant, produced by [Case] or [WrappedCase].
type CaseDef struct{ c engine.Case }

// Case declares a variant case: the payload type T, carried under name on the
// wire.
func Case[T any](name string) CaseDef {
	return CaseDef{engine.Case{Name: name, Typ: reflect.TypeFor[T]()}}
}

// WrappedCase declares a variant case whose payload is the single field of T,
// rather than T itself:
//
//	type EventAt struct{ Value time.Time }
//	func (EventAt) isEvent() {}
//
//	var _ = golem.DefineVariant[Event](golem.WrappedCase[EventAt]("at"))
//
// publishes `at(datetime)`. Reach for it when the payload cannot itself carry
// the variant's marker method without losing what it is: a time.Time, a
// golem.Text, an Option or Result, another variant. A defined type over any of
// those — `type EventAt time.Time` — is a different type to the SDK, and would
// publish a different schema.
func WrappedCase[T any](name string) CaseDef {
	return CaseDef{engine.Case{Name: name, Typ: reflect.TypeFor[T](), Wrapped: true}}
}

// DefineVariant registers the closed set of types inhabiting the interface
// Iface. Case order is declaration order, and it is the wire order — reordering
// the arguments is a breaking change. Call it from a package-level var so
// registration happens before the component is invoked.
func DefineVariant[Iface any](cases ...CaseDef) *engine.VariantDef {
	return defineVariantInto[Iface](defs, cases...)
}

// defineVariantInto is the instance-scoped implementation behind DefineVariant.
func defineVariantInto[Iface any](d *definitions, cases ...CaseDef) *engine.VariantDef {
	cs := make([]engine.Case, len(cases))
	for i, c := range cases {
		cs[i] = c.c
	}
	return d.DefineVariant(reflect.TypeFor[Iface](), cs)
}

// ---------------------------------------------------------------------------
// enums
// ---------------------------------------------------------------------------

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
// Values are positional: Status(0) is "active", so the constants are expected to
// run 0..n-1 in declaration order. A value outside 0..len(names)-1 is rejected
// at encode time rather than silently truncated.
func DefineEnum[T any](names ...string) *engine.EnumDef {
	return defineEnumInto[T](defs, names...)
}

// defineEnumInto is the instance-scoped implementation behind DefineEnum.
func defineEnumInto[T any](d *definitions, names ...string) *engine.EnumDef {
	return d.DefineEnum(reflect.TypeFor[T](), names)
}
