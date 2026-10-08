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

	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// encodeParams encodes a parameter list — a record whose fields are the
// parameters, in declaration order.
func encodeParams(fields []engine.Field, v reflect.Value) types.SchemaValueTree {
	var b engine.ValBuilder
	idxs := make([]int32, 0, len(fields))
	for _, f := range fields {
		if f.AutoInjected {
			continue // filled by the host, never sent
		}
		idxs = append(idxs, f.Codec.Encode(&b, v.Field(f.Index)))
	}
	root := b.Push(types.MakeSchemaValueNodeRecordValue(idxs))
	return types.SchemaValueTree{ValueNodes: b.Nodes, Root: root}
}

// decodeParams decodes a parameter-list tree (a record at the root) into dst's
// fields. An empty parameter list is permitted to arrive as an empty record.
// Principal fields have no value in the tree: they are set to principal, which
// may be nil where there is no invocation (parsing an agent id).
func decodeParams(tree types.SchemaValueTree, fields []engine.Field, dst reflect.Value, principal Principal) error {
	if principal != nil {
		for _, f := range principalFields(fields) {
			dst.Field(f.Index).Set(reflect.ValueOf(&principal).Elem())
		}
	}
	fields = userFields(fields)
	d := engine.Decoder{Nodes: tree.ValueNodes}
	if len(d.Nodes) == 0 {
		if len(fields) == 0 {
			return nil
		}
		return fmt.Errorf("empty value tree but %d parameter(s) expected", len(fields))
	}
	root, err := d.Node(tree.Root)
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
		if err := f.Codec.Decode(&d, dst.Field(f.Index), idxs[i]); err != nil {
			return fmt.Errorf("parameter %q: %w", f.Name, err)
		}
	}
	return nil
}
