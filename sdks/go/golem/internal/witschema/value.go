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

package witschema

import (
	"fmt"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Value conversion, both ways.
//
// The flat form addresses children by index into one pool; the recursive form
// holds them directly. Converting is total: a value is built in one pass so a
// failure part-way through has a single owner that can release the
// host-managed handles already taken.

// ValueToCore converts a generated value tree to the shared model.
func ValueToCore(tree types.SchemaValueTree) (core.SchemaValue, error) {
	v := &valueConverter{tree: tree}
	return v.node(tree.Root)
}

type valueConverter struct {
	tree  types.SchemaValueTree
	depth int
}

func (c *valueConverter) node(idx int32) (core.SchemaValue, error) {
	if idx < 0 || int(idx) >= len(c.tree.ValueNodes) {
		return nil, fmt.Errorf("golem: value node index %d is out of range (%d nodes)",
			idx, len(c.tree.ValueNodes))
	}
	if c.depth >= maxDepth {
		return nil, fmt.Errorf("golem: value nests deeper than %d levels", maxDepth)
	}
	c.depth++
	defer func() { c.depth-- }()

	n := c.tree.ValueNodes[idx]
	switch n.Tag() {
	case types.SchemaValueNodeBoolValue:
		return core.BoolValue{Value: n.BoolValue()}, nil
	case types.SchemaValueNodeS8Value:
		return core.S8Value{Value: n.S8Value()}, nil
	case types.SchemaValueNodeS16Value:
		return core.S16Value{Value: n.S16Value()}, nil
	case types.SchemaValueNodeS32Value:
		return core.S32Value{Value: n.S32Value()}, nil
	case types.SchemaValueNodeS64Value:
		return core.S64Value{Value: n.S64Value()}, nil
	case types.SchemaValueNodeU8Value:
		return core.U8Value{Value: n.U8Value()}, nil
	case types.SchemaValueNodeU16Value:
		return core.U16Value{Value: n.U16Value()}, nil
	case types.SchemaValueNodeU32Value:
		return core.U32Value{Value: n.U32Value()}, nil
	case types.SchemaValueNodeU64Value:
		return core.U64Value{Value: n.U64Value()}, nil
	case types.SchemaValueNodeF32Value:
		return core.F32Value{Value: n.F32Value()}, nil
	case types.SchemaValueNodeF64Value:
		return core.F64Value{Value: n.F64Value()}, nil
	case types.SchemaValueNodeCharValue:
		return core.CharValue{Value: n.CharValue()}, nil
	case types.SchemaValueNodeStringValue:
		return core.StringValue{Value: n.StringValue()}, nil
	case types.SchemaValueNodePathValue:
		return core.PathValue{Value: n.PathValue()}, nil
	case types.SchemaValueNodeUrlValue:
		return core.UrlValue{Value: n.UrlValue()}, nil

	case types.SchemaValueNodeDatetimeValue:
		d := n.DatetimeValue()
		return core.DatetimeValue{Seconds: d.Seconds, Nanoseconds: d.Nanoseconds}, nil
	case types.SchemaValueNodeDurationValue:
		return core.DurationValue{Nanoseconds: n.DurationValue().Nanoseconds}, nil
	case types.SchemaValueNodeQuantityValueNode:
		q := n.QuantityValueNode()
		return core.QuantityValueNode{Value: core.QuantityValue{
			Mantissa: q.Mantissa, Scale: q.Scale, Unit: q.Unit,
		}}, nil

	case types.SchemaValueNodeTextValue:
		p := n.TextValue()
		return core.TextValue{Text: p.Text, Language: optVal(p.Language)}, nil
	case types.SchemaValueNodeBinaryValue:
		p := n.BinaryValue()
		return core.BinaryValue{
			Bytes: append([]byte(nil), p.Bytes...), MimeType: optVal(p.MimeType),
		}, nil

	case types.SchemaValueNodeRecordValue:
		fields, err := c.each(n.RecordValue())
		return core.RecordValue{Fields: fields}, err
	case types.SchemaValueNodeTupleValue:
		elems, err := c.each(n.TupleValue())
		return core.TupleValue{Elements: elems}, err
	case types.SchemaValueNodeListValue:
		items, err := c.each(n.ListValue())
		return core.ListValue{Items: items}, err
	case types.SchemaValueNodeFixedListValue:
		items, err := c.each(n.FixedListValue())
		return core.FixedListValue{Items: items}, err

	case types.SchemaValueNodeVariantValue:
		p := n.VariantValue()
		payload, err := c.optNode(p.Payload)
		return core.VariantValue{Case: p.Case, Payload: payload}, err
	case types.SchemaValueNodeEnumValue:
		return core.EnumValue{Case: n.EnumValue()}, nil
	case types.SchemaValueNodeFlagsValue:
		return core.FlagsValue{Set: append([]bool(nil), n.FlagsValue()...)}, nil

	case types.SchemaValueNodeMapValue:
		entries := make([]core.MapEntry, 0, len(n.MapValue()))
		for _, e := range n.MapValue() {
			k, err := c.node(e.Key)
			if err != nil {
				return nil, err
			}
			v, err := c.node(e.Value)
			if err != nil {
				return nil, err
			}
			entries = append(entries, core.MapEntry{Key: k, Value: v})
		}
		return core.MapValue{Entries: entries}, nil

	case types.SchemaValueNodeOptionValue:
		inner, err := c.optNode(n.OptionValue())
		return core.OptionValue{Value: inner}, err

	case types.SchemaValueNodeResultValue:
		p := n.ResultValue()
		if p.Tag() == types.ResultValuePayloadOkValue {
			inner, err := c.optNode(p.OkValue())
			return core.ResultValue{Value: inner}, err
		}
		inner, err := c.optNode(p.ErrValue())
		return core.ResultValue{IsErr: true, Value: inner}, err

	case types.SchemaValueNodeUnionValue:
		p := n.UnionValue()
		body, err := c.node(p.Body)
		return core.UnionValue{Tag: p.Tag, Body: body}, err

	case types.SchemaValueNodeSecretValue,
		types.SchemaValueNodeQuotaTokenHandle,
		types.SchemaValueNodePermissionCardHandle,
		types.SchemaValueNodeStreamValue:
		return handleToCore(n)
	}
	return nil, fmt.Errorf("golem: unknown value node (tag %d); the SDK's bindings may be out of date", n.Tag())
}

func (c *valueConverter) each(idxs []int32) ([]core.SchemaValue, error) {
	out := make([]core.SchemaValue, 0, len(idxs))
	for _, idx := range idxs {
		v, err := c.node(idx)
		if err != nil {
			return nil, err
		}
		out = append(out, v)
	}
	return out, nil
}

func (c *valueConverter) optNode(o witTypes.Option[int32]) (*core.SchemaValue, error) {
	if o.IsNone() {
		return nil, nil
	}
	v, err := c.node(o.Some())
	if err != nil {
		return nil, err
	}
	return &v, nil
}

// ValueToWit flattens a shared-model value into a generated value tree.
//
// On failure the handles already taken are released, because the caller has no
// other reference to them: the partially built tree is discarded here.
func ValueToWit(v core.SchemaValue) (types.SchemaValueTree, error) {
	f := &flattener{}
	root, err := f.push(v)
	if err != nil {
		core.ReleaseAll(v)
		return types.SchemaValueTree{}, err
	}
	return types.SchemaValueTree{ValueNodes: f.nodes, Root: root}, nil
}

type flattener struct {
	nodes []types.SchemaValueNode
	depth int
}

func (f *flattener) add(n types.SchemaValueNode) int32 {
	f.nodes = append(f.nodes, n)
	return int32(len(f.nodes) - 1)
}

func (f *flattener) push(v core.SchemaValue) (int32, error) {
	if f.depth >= maxDepth {
		return 0, fmt.Errorf("golem: value nests deeper than %d levels", maxDepth)
	}
	f.depth++
	defer func() { f.depth-- }()

	switch t := v.(type) {
	case core.BoolValue:
		return f.add(types.MakeSchemaValueNodeBoolValue(t.Value)), nil
	case core.S8Value:
		return f.add(types.MakeSchemaValueNodeS8Value(t.Value)), nil
	case core.S16Value:
		return f.add(types.MakeSchemaValueNodeS16Value(t.Value)), nil
	case core.S32Value:
		return f.add(types.MakeSchemaValueNodeS32Value(t.Value)), nil
	case core.S64Value:
		return f.add(types.MakeSchemaValueNodeS64Value(t.Value)), nil
	case core.U8Value:
		return f.add(types.MakeSchemaValueNodeU8Value(t.Value)), nil
	case core.U16Value:
		return f.add(types.MakeSchemaValueNodeU16Value(t.Value)), nil
	case core.U32Value:
		return f.add(types.MakeSchemaValueNodeU32Value(t.Value)), nil
	case core.U64Value:
		return f.add(types.MakeSchemaValueNodeU64Value(t.Value)), nil
	case core.F32Value:
		return f.add(types.MakeSchemaValueNodeF32Value(t.Value)), nil
	case core.F64Value:
		return f.add(types.MakeSchemaValueNodeF64Value(t.Value)), nil
	case core.CharValue:
		return f.add(types.MakeSchemaValueNodeCharValue(t.Value)), nil
	case core.StringValue:
		return f.add(types.MakeSchemaValueNodeStringValue(t.Value)), nil
	case core.PathValue:
		return f.add(types.MakeSchemaValueNodePathValue(t.Value)), nil
	case core.UrlValue:
		return f.add(types.MakeSchemaValueNodeUrlValue(t.Value)), nil

	case core.DatetimeValue:
		return f.add(types.MakeSchemaValueNodeDatetimeValue(types.Datetime{
			Seconds: t.Seconds, Nanoseconds: t.Nanoseconds,
		})), nil
	case core.DurationValue:
		return f.add(types.MakeSchemaValueNodeDurationValue(types.DurationValuePayload{
			Nanoseconds: t.Nanoseconds,
		})), nil
	case core.QuantityValueNode:
		return f.add(types.MakeSchemaValueNodeQuantityValueNode(types.QuantityValue{
			Mantissa: t.Value.Mantissa, Scale: t.Value.Scale, Unit: t.Value.Unit,
		})), nil

	case core.TextValue:
		return f.add(types.MakeSchemaValueNodeTextValue(types.TextValuePayload{
			Text: t.Text, Language: witOpt(t.Language),
		})), nil
	case core.BinaryValue:
		return f.add(types.MakeSchemaValueNodeBinaryValue(types.BinaryValuePayload{
			Bytes: append([]uint8(nil), t.Bytes...), MimeType: witOpt(t.MimeType),
		})), nil

	case core.RecordValue:
		idxs, err := f.each(t.Fields)
		if err != nil {
			return 0, err
		}
		return f.add(types.MakeSchemaValueNodeRecordValue(idxs)), nil
	case core.TupleValue:
		idxs, err := f.each(t.Elements)
		if err != nil {
			return 0, err
		}
		return f.add(types.MakeSchemaValueNodeTupleValue(idxs)), nil
	case core.ListValue:
		idxs, err := f.each(t.Items)
		if err != nil {
			return 0, err
		}
		return f.add(types.MakeSchemaValueNodeListValue(idxs)), nil
	case core.FixedListValue:
		idxs, err := f.each(t.Items)
		if err != nil {
			return 0, err
		}
		return f.add(types.MakeSchemaValueNodeFixedListValue(idxs)), nil

	case core.VariantValue:
		payload, err := f.optPush(t.Payload)
		if err != nil {
			return 0, err
		}
		return f.add(types.MakeSchemaValueNodeVariantValue(types.VariantValuePayload{
			Case: t.Case, Payload: payload,
		})), nil
	case core.EnumValue:
		return f.add(types.MakeSchemaValueNodeEnumValue(t.Case)), nil
	case core.FlagsValue:
		return f.add(types.MakeSchemaValueNodeFlagsValue(append([]bool(nil), t.Set...))), nil

	case core.MapValue:
		entries := make([]types.MapEntry, 0, len(t.Entries))
		for _, e := range t.Entries {
			k, err := f.push(e.Key)
			if err != nil {
				return 0, err
			}
			v, err := f.push(e.Value)
			if err != nil {
				return 0, err
			}
			entries = append(entries, types.MapEntry{Key: k, Value: v})
		}
		return f.add(types.MakeSchemaValueNodeMapValue(entries)), nil

	case core.OptionValue:
		inner, err := f.optPush(t.Value)
		if err != nil {
			return 0, err
		}
		return f.add(types.MakeSchemaValueNodeOptionValue(inner)), nil

	case core.ResultValue:
		inner, err := f.optPush(t.Value)
		if err != nil {
			return 0, err
		}
		if t.IsErr {
			return f.add(types.MakeSchemaValueNodeResultValue(
				types.MakeResultValuePayloadErrValue(inner))), nil
		}
		return f.add(types.MakeSchemaValueNodeResultValue(
			types.MakeResultValuePayloadOkValue(inner))), nil

	case core.UnionValue:
		body, err := f.push(t.Body)
		if err != nil {
			return 0, err
		}
		return f.add(types.MakeSchemaValueNodeUnionValue(types.UnionValuePayload{
			Tag: t.Tag, Body: body,
		})), nil

	case core.SecretValue, core.QuotaTokenValue, core.PermissionCardValue, core.StreamValue:
		n, err := handleToWit(v)
		if err != nil {
			return 0, err
		}
		return f.add(n), nil
	}
	return 0, fmt.Errorf("golem: cannot convert %T to a value node", v)
}

func (f *flattener) each(vs []core.SchemaValue) ([]int32, error) {
	out := make([]int32, 0, len(vs))
	for _, v := range vs {
		idx, err := f.push(v)
		if err != nil {
			return nil, err
		}
		out = append(out, idx)
	}
	return out, nil
}

func (f *flattener) optPush(v *core.SchemaValue) (witTypes.Option[int32], error) {
	if v == nil {
		return witTypes.None[int32](), nil
	}
	idx, err := f.push(*v)
	if err != nil {
		return witTypes.None[int32](), err
	}
	return witTypes.Some(idx), nil
}

func witOpt[T any](p *T) witTypes.Option[T] {
	if p == nil {
		return witTypes.None[T]()
	}
	return witTypes.Some(*p)
}
