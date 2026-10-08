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

package engine

import (
	"fmt"
	"reflect"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// compileTuple lowers a TupleN to the WIT tuple type. The struct's fields are
// the elements in order, so both directions walk them positionally.
func compileTuple(c *Codec, elems []*Codec) {
	c.Body = func(g *GraphBuilder) types.SchemaTypeBody {
		idx := make([]int32, len(elems))
		for i, e := range elems {
			idx[i] = g.Node(e)
		}
		return types.MakeSchemaTypeBodyTupleType(idx)
	}
	c.Encode = func(b *ValBuilder, v reflect.Value) int32 {
		idx := make([]int32, len(elems))
		for i, e := range elems {
			idx[i] = e.Encode(b, v.Field(i))
		}
		return b.Push(types.MakeSchemaValueNodeTupleValue(idx))
	}
	c.Decode = func(d *Decoder, dst reflect.Value, idx int32) error {
		n, err := d.Node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeTupleValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.Typ)
		}
		items := n.TupleValue()
		if len(items) != len(elems) {
			return fmt.Errorf("%s: tuple value has %d elements, but %s has %d",
				c.Typ, len(items), c.Typ, len(elems))
		}
		for i, e := range elems {
			if err := e.Decode(d, dst.Field(i), items[i]); err != nil {
				return err
			}
		}
		return nil
	}
}
