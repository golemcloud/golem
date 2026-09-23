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

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Tuples are positional, unnamed groups, lowering to the WIT tuple type. Go has
// no tuple, so the SDK provides one type per arity:
//
//	func (a *Agent) Split(ctx *golem.Context[S], in string) golem.Tuple2[string, int64]
//
// A tuple differs from a record on the wire: its elements are addressed by
// position and carry no names. Prefer a struct when the parts have meaningful
// names; reach for a tuple when they genuinely do not, or when mirroring a
// foreign interface that uses one.

// tupleish reports the element types of a tuple, resolved once at compile time
// against the zero value. The elements are the struct's fields in declaration
// order, so encoding and decoding walk the fields positionally.
type tupleish interface{ tupleElems() []reflect.Type }

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

// compileTuple lowers a TupleN to the WIT tuple type. The struct's fields are
// the elements in order, so both directions walk them positionally.
func compileTuple(c *codec, elems []*codec) {
	c.body = func(g *graphBuilder) types.SchemaTypeBody {
		idx := make([]int32, len(elems))
		for i, e := range elems {
			idx[i] = g.node(e)
		}
		return types.MakeSchemaTypeBodyTupleType(idx)
	}
	c.encode = func(b *valBuilder, v reflect.Value) int32 {
		idx := make([]int32, len(elems))
		for i, e := range elems {
			idx[i] = e.encode(b, v.Field(i))
		}
		return b.push(types.MakeSchemaValueNodeTupleValue(idx))
	}
	c.decode = func(d *decoder, dst reflect.Value, idx int32) error {
		n, err := d.node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeTupleValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.typ)
		}
		items := n.TupleValue()
		if len(items) != len(elems) {
			return fmt.Errorf("%s: tuple value has %d elements, but %s has %d",
				c.typ, len(items), c.typ, len(elems))
		}
		for i, e := range elems {
			if err := e.decode(d, dst.Field(i), items[i]); err != nil {
				return err
			}
		}
		return nil
	}
}
