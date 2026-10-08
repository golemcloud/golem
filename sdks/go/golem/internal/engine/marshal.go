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

// Values cross the boundary as a schema-value-tree: a flat pool of nodes plus
// the index of the root. This file holds only the node-level primitives; how a
// given Go type maps onto them lives in its codec (see codec.go).

// ---------------------------------------------------------------------------
// encode
// ---------------------------------------------------------------------------

type ValBuilder struct{ Nodes []types.SchemaValueNode }

func (b *ValBuilder) Push(n types.SchemaValueNode) int32 {
	b.Nodes = append(b.Nodes, n)
	return int32(len(b.Nodes) - 1)
}

// EncodeWith encodes v using c, producing a complete value tree.
func EncodeWith(c *Codec, v reflect.Value) types.SchemaValueTree {
	var b ValBuilder
	root := c.Encode(&b, v)
	return types.SchemaValueTree{ValueNodes: b.Nodes, Root: root}
}

// ---------------------------------------------------------------------------
// decode
// ---------------------------------------------------------------------------

type Decoder struct{ Nodes []types.SchemaValueNode }

func (d *Decoder) Node(idx int32) (types.SchemaValueNode, error) {
	if idx < 0 || int(idx) >= len(d.Nodes) {
		return types.SchemaValueNode{}, fmt.Errorf("value node index %d out of range (%d nodes)", idx, len(d.Nodes))
	}
	return d.Nodes[idx], nil
}
