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

	"github.com/golemcloud/golem/sdks/go/core/values"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Tuples are positional, unnamed groups, lowering to the WIT tuple type. The
// types live in the shared core module; see [values.Tuple2].

// Tuple2 is a 2-element tuple.
type Tuple2[A, B any] = values.Tuple2[A, B]

// Tuple3 is a 3-element tuple.
type Tuple3[A, B, C any] = values.Tuple3[A, B, C]

// Tuple4 is a 4-element tuple.
type Tuple4[A, B, C, D any] = values.Tuple4[A, B, C, D]

// Tuple5 is a 5-element tuple.
type Tuple5[A, B, C, D, E any] = values.Tuple5[A, B, C, D, E]

// Tuple6 is a 6-element tuple.
type Tuple6[A, B, C, D, E, F any] = values.Tuple6[A, B, C, D, E, F]

// Tuple7 is a 7-element tuple.
type Tuple7[A, B, C, D, E, F, G any] = values.Tuple7[A, B, C, D, E, F, G]

// Tuple8 is an 8-element tuple.
type Tuple8[A, B, C, D, E, F, G, H any] = values.Tuple8[A, B, C, D, E, F, G, H]

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
