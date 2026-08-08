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

// Values cross the boundary as a schema-value-tree: a flat pool of nodes plus
// the index of the root. This file holds only the node-level primitives; how a
// given Go type maps onto them lives in its codec (see codec.go).

// ---------------------------------------------------------------------------
// encode
// ---------------------------------------------------------------------------

type valBuilder struct{ nodes []types.SchemaValueNode }

func (b *valBuilder) push(n types.SchemaValueNode) int32 {
	b.nodes = append(b.nodes, n)
	return int32(len(b.nodes) - 1)
}

// encodeWith encodes v using c, producing a complete value tree.
func encodeWith(c *codec, v reflect.Value) types.SchemaValueTree {
	var b valBuilder
	root := c.encode(&b, v)
	return types.SchemaValueTree{ValueNodes: b.nodes, Root: root}
}

// encodeParams encodes a parameter list — a record whose fields are the
// parameters, in declaration order.
func encodeParams(fields []fieldInfo, v reflect.Value) types.SchemaValueTree {
	var b valBuilder
	idxs := make([]int32, 0, len(fields))
	for _, f := range fields {
		idxs = append(idxs, f.codec.encode(&b, v.Field(f.index)))
	}
	root := b.push(types.MakeSchemaValueNodeRecordValue(idxs))
	return types.SchemaValueTree{ValueNodes: b.nodes, Root: root}
}

// ---------------------------------------------------------------------------
// decode
// ---------------------------------------------------------------------------

type decoder struct{ nodes []types.SchemaValueNode }

func (d *decoder) node(idx int32) (types.SchemaValueNode, error) {
	if idx < 0 || int(idx) >= len(d.nodes) {
		return types.SchemaValueNode{}, fmt.Errorf("value node index %d out of range (%d nodes)", idx, len(d.nodes))
	}
	return d.nodes[idx], nil
}

// decodeParams decodes a parameter-list tree (a record at the root) into dst's
// fields. An empty parameter list is permitted to arrive as an empty record.
func decodeParams(tree types.SchemaValueTree, fields []fieldInfo, dst reflect.Value) error {
	d := decoder{nodes: tree.ValueNodes}
	if len(d.nodes) == 0 {
		if len(fields) == 0 {
			return nil
		}
		return fmt.Errorf("empty value tree but %d parameter(s) expected", len(fields))
	}
	root, err := d.node(tree.Root)
	if err != nil {
		return err
	}
	if root.Tag() != types.SchemaValueNodeRecordValue {
		if len(fields) == 0 {
			return nil
		}
		return fmt.Errorf("expected a record at the root of the parameter list")
	}
	idxs := root.RecordValue()
	if len(idxs) < len(fields) {
		return fmt.Errorf("parameter list has %d value(s), want %d", len(idxs), len(fields))
	}
	for i, f := range fields {
		if err := f.codec.decode(&d, dst.Field(f.index), idxs[i]); err != nil {
			return fmt.Errorf("parameter %q: %w", f.name, err)
		}
	}
	return nil
}
