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

import "reflect"

// Tuples are positional, unnamed groups, lowering to the WIT tuple type. Go has
// no tuple, so the SDK provides one type per arity:
//
//	func (a *Agent) Split(ctx *golem.Context[S], in string) golem.Tuple2[string, int64]
//
// A tuple differs from a record on the wire: its elements are addressed by
// position and carry no names. Prefer a struct when the parts have meaningful
// names; reach for a tuple when they genuinely do not, or when mirroring a
// foreign interface that uses one.

// Tupleish reports the element types of a tuple, resolved once at compile time
// against the zero value. Sealed like the other plumbing interfaces. The elements are the struct's fields in declaration
// order, so encoding and decoding walk the fields positionally.
type Tupleish interface{ tupleElems() []reflect.Type }

// Tuple2 is a 2-element tuple.
type Tuple2[A, B any] struct {
	A A
	B B
}

func (t Tuple2[A, B]) tupleElems() []reflect.Type {
	return []reflect.Type{reflect.TypeFor[A](), reflect.TypeFor[B]()}
}

// Tuple3 is a 3-element tuple.
type Tuple3[A, B, C any] struct {
	A A
	B B
	C C
}

func (t Tuple3[A, B, C]) tupleElems() []reflect.Type {
	return []reflect.Type{reflect.TypeFor[A](), reflect.TypeFor[B](), reflect.TypeFor[C]()}
}

// Tuple4 is a 4-element tuple.
type Tuple4[A, B, C, D any] struct {
	A A
	B B
	C C
	D D
}

func (t Tuple4[A, B, C, D]) tupleElems() []reflect.Type {
	return []reflect.Type{reflect.TypeFor[A](), reflect.TypeFor[B](), reflect.TypeFor[C](), reflect.TypeFor[D]()}
}

// Tuple5 is a 5-element tuple.
type Tuple5[A, B, C, D, E any] struct {
	A A
	B B
	C C
	D D
	E E
}

func (t Tuple5[A, B, C, D, E]) tupleElems() []reflect.Type {
	return []reflect.Type{reflect.TypeFor[A](), reflect.TypeFor[B](), reflect.TypeFor[C](), reflect.TypeFor[D](), reflect.TypeFor[E]()}
}

// Tuple6 is a 6-element tuple.
type Tuple6[A, B, C, D, E, F any] struct {
	A A
	B B
	C C
	D D
	E E
	F F
}

func (t Tuple6[A, B, C, D, E, F]) tupleElems() []reflect.Type {
	return []reflect.Type{reflect.TypeFor[A](), reflect.TypeFor[B](), reflect.TypeFor[C](), reflect.TypeFor[D](), reflect.TypeFor[E](), reflect.TypeFor[F]()}
}

// Tuple7 is a 7-element tuple.
type Tuple7[A, B, C, D, E, F, G any] struct {
	A A
	B B
	C C
	D D
	E E
	F F
	G G
}

func (t Tuple7[A, B, C, D, E, F, G]) tupleElems() []reflect.Type {
	return []reflect.Type{reflect.TypeFor[A](), reflect.TypeFor[B](), reflect.TypeFor[C](), reflect.TypeFor[D](), reflect.TypeFor[E](), reflect.TypeFor[F](), reflect.TypeFor[G]()}
}

// Tuple8 is a 8-element tuple.
type Tuple8[A, B, C, D, E, F, G, H any] struct {
	A A
	B B
	C C
	D D
	E E
	F F
	G G
	H H
}

func (t Tuple8[A, B, C, D, E, F, G, H]) tupleElems() []reflect.Type {
	return []reflect.Type{reflect.TypeFor[A](), reflect.TypeFor[B](), reflect.TypeFor[C](), reflect.TypeFor[D](), reflect.TypeFor[E](), reflect.TypeFor[F](), reflect.TypeFor[G](), reflect.TypeFor[H]()}
}

// TupleElems reports a tuple's element types in declaration order. See the
// codec-plumbing note in values.go for why this is a function rather than a
// method.
func TupleElems(v any) ([]reflect.Type, bool) {
	t, ok := v.(Tupleish)
	if !ok {
		return nil, false
	}
	return t.tupleElems(), true
}
