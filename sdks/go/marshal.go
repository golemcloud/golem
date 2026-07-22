// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
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

	types "github.com/golemcloud/golem-go/internal/wit/golem_core_types"
)

// Values cross the boundary as a schema-value-tree: a flat pool of nodes plus
// the index of the root. Records are positional — field order is declaration
// order, which is also what the schema derivation reports.
//
// Encoding is type-directed (driven by the declared Go type, not the runtime
// value) so that declared interface types can select variant encoding later.
//
// Decoding is deliberately defensive: the generated accessors panic on tag
// mismatch, so every access checks Tag() first and returns an error instead.

// ---------------------------------------------------------------------------
// encode
// ---------------------------------------------------------------------------

type valBuilder struct{ nodes []types.SchemaValueNode }

func (b *valBuilder) push(n types.SchemaValueNode) int32 {
	b.nodes = append(b.nodes, n)
	return int32(len(b.nodes) - 1)
}

func (b *valBuilder) addTyped(t reflect.Type, v reflect.Value) int32 {
	if v.Kind() == reflect.Interface {
		v = v.Elem()
	}
	switch v.Kind() {
	case reflect.String:
		return b.push(types.MakeSchemaValueNodeStringValue(v.String()))
	case reflect.Bool:
		return b.push(types.MakeSchemaValueNodeBoolValue(v.Bool()))
	case reflect.Int, reflect.Int64:
		return b.push(types.MakeSchemaValueNodeS64Value(v.Int()))
	case reflect.Int32:
		return b.push(types.MakeSchemaValueNodeS32Value(int32(v.Int())))
	case reflect.Int16:
		return b.push(types.MakeSchemaValueNodeS16Value(int16(v.Int())))
	case reflect.Int8:
		return b.push(types.MakeSchemaValueNodeS8Value(int8(v.Int())))
	case reflect.Uint, reflect.Uint64:
		return b.push(types.MakeSchemaValueNodeU64Value(v.Uint()))
	case reflect.Uint32:
		return b.push(types.MakeSchemaValueNodeU32Value(uint32(v.Uint())))
	case reflect.Uint16:
		return b.push(types.MakeSchemaValueNodeU16Value(uint16(v.Uint())))
	case reflect.Uint8:
		return b.push(types.MakeSchemaValueNodeU8Value(uint8(v.Uint())))
	case reflect.Float64:
		return b.push(types.MakeSchemaValueNodeF64Value(v.Float()))
	case reflect.Float32:
		return b.push(types.MakeSchemaValueNodeF32Value(float32(v.Float())))
	case reflect.Struct:
		// Children are appended first; the record node then refers to them.
		var idxs []int32
		for i := range v.NumField() {
			f := v.Type().Field(i)
			if f.PkgPath != "" {
				continue
			}
			idxs = append(idxs, b.addTyped(f.Type, v.Field(i)))
		}
		return b.push(types.MakeSchemaValueNodeRecordValue(idxs))
	default:
		panic(fmt.Sprintf("golem: cannot encode Go type %s (kind %s)", t, v.Kind()))
	}
}

// encodeTyped encodes v, whose declared type is t, as a value tree.
func encodeTyped(t reflect.Type, v reflect.Value) types.SchemaValueTree {
	var b valBuilder
	root := b.addTyped(t, v)
	return types.SchemaValueTree{ValueNodes: b.nodes, Root: root}
}

// encodeParams encodes a struct as a method/constructor parameter list, i.e. a
// record whose fields are the parameters in declaration order.
func encodeParams(t reflect.Type, v reflect.Value) types.SchemaValueTree {
	return encodeTyped(t, v)
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

// into decodes the node at idx into dst, whose declared type is t.
func (d *decoder) into(t reflect.Type, dst reflect.Value, idx int32) error {
	n, err := d.node(idx)
	if err != nil {
		return err
	}
	tag := n.Tag()

	// Every branch checks Tag() first: the generated accessors panic on
	// mismatch, and malformed input must produce an error, not a panic.
	switch t.Kind() {
	case reflect.String:
		if tag == types.SchemaValueNodeStringValue {
			dst.SetString(n.StringValue())
			return nil
		}
	case reflect.Bool:
		if tag == types.SchemaValueNodeBoolValue {
			dst.SetBool(n.BoolValue())
			return nil
		}
	case reflect.Int, reflect.Int64:
		if tag == types.SchemaValueNodeS64Value {
			dst.SetInt(n.S64Value())
			return nil
		}
	case reflect.Int32:
		if tag == types.SchemaValueNodeS32Value {
			dst.SetInt(int64(n.S32Value()))
			return nil
		}
	case reflect.Int16:
		if tag == types.SchemaValueNodeS16Value {
			dst.SetInt(int64(n.S16Value()))
			return nil
		}
	case reflect.Int8:
		if tag == types.SchemaValueNodeS8Value {
			dst.SetInt(int64(n.S8Value()))
			return nil
		}
	case reflect.Uint, reflect.Uint64:
		if tag == types.SchemaValueNodeU64Value {
			dst.SetUint(n.U64Value())
			return nil
		}
	case reflect.Uint32:
		if tag == types.SchemaValueNodeU32Value {
			dst.SetUint(uint64(n.U32Value()))
			return nil
		}
	case reflect.Uint16:
		if tag == types.SchemaValueNodeU16Value {
			dst.SetUint(uint64(n.U16Value()))
			return nil
		}
	case reflect.Uint8:
		if tag == types.SchemaValueNodeU8Value {
			dst.SetUint(uint64(n.U8Value()))
			return nil
		}
	case reflect.Float64:
		if tag == types.SchemaValueNodeF64Value {
			dst.SetFloat(n.F64Value())
			return nil
		}
	case reflect.Float32:
		if tag == types.SchemaValueNodeF32Value {
			dst.SetFloat(float64(n.F32Value()))
			return nil
		}
	case reflect.Struct:
		if tag != types.SchemaValueNodeRecordValue {
			break
		}
		idxs := n.RecordValue()
		fields := structFields(t)
		if len(idxs) < len(fields) {
			return fmt.Errorf("record for %s has %d field(s), want %d", t, len(idxs), len(fields))
		}
		for i, f := range fields {
			if err := d.into(f.typ, dst.Field(f.index), idxs[i]); err != nil {
				return fmt.Errorf("%s.%s: %w", t, f.name, err)
			}
		}
		return nil
	}
	return fmt.Errorf("cannot decode value node (tag %d) into %s", tag, t)
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
		if err := d.into(f.typ, dst.Field(f.index), idxs[i]); err != nil {
			return fmt.Errorf("parameter %q: %w", f.name, err)
		}
	}
	return nil
}
