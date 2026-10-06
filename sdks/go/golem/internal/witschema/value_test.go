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
	"reflect"
	"strings"
	"testing"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// valueBuilder assembles a flat WIT value tree the way the generated code does.
type valueBuilder struct{ nodes []types.SchemaValueNode }

func (b *valueBuilder) push(n types.SchemaValueNode) int32 {
	b.nodes = append(b.nodes, n)
	return int32(len(b.nodes) - 1)
}

func (b *valueBuilder) tree(root int32) types.SchemaValueTree {
	return types.SchemaValueTree{ValueNodes: b.nodes, Root: root}
}

// TestValueRoundTripsThroughCore is the property the guest SDK depends on:
// flattening what was unflattened gives back the same tree, so nothing is lost
// crossing the boundary in either direction.
func TestValueRoundTripsThroughCore(t *testing.T) {
	var b valueBuilder
	name := b.push(types.MakeSchemaValueNodeStringValue("ada"))
	count := b.push(types.MakeSchemaValueNodeS64Value(-9223372036854775808))
	tag := b.push(types.MakeSchemaValueNodeStringValue("x"))
	inner := b.push(types.MakeSchemaValueNodeOptionValue(witTypes.Some(tag)))
	items := b.push(types.MakeSchemaValueNodeListValue([]int32{name, tag}))
	entry := b.push(types.MakeSchemaValueNodeStringValue("v"))
	amap := b.push(types.MakeSchemaValueNodeMapValue([]types.MapEntry{{Key: name, Value: entry}}))
	res := b.push(types.MakeSchemaValueNodeResultValue(
		types.MakeResultValuePayloadErrValue(witTypes.Some(entry))))
	id := b.push(types.MakeSchemaValueNodeUuidValue(types.Uuid{HighBits: 0xdd00721b33294621, LowBits: 0xa01dc71f02cd78c6}))
	root := b.push(types.MakeSchemaValueNodeRecordValue([]int32{name, count, inner, items, amap, res, id}))

	original := b.tree(root)
	asCore, err := ValueToCore(original)
	if err != nil {
		t.Fatalf("ValueToCore: %v", err)
	}
	back, err := ValueToWit(asCore)
	if err != nil {
		t.Fatalf("ValueToWit: %v", err)
	}

	// The node pool is rebuilt, so compare what the trees mean rather than how
	// they are laid out: convert once more and check the core forms match.
	again, err := ValueToCore(back)
	if err != nil {
		t.Fatalf("ValueToCore (second): %v", err)
	}
	if !reflect.DeepEqual(asCore, again) {
		t.Errorf("a round trip changed the value:\n got: %#v\nwant: %#v", again, asCore)
	}
	if got := asCore.(core.RecordValue).Fields[6].(core.UUIDValue).Value.String(); got != "dd00721b-3329-4621-a01d-c71f02cd78c6" {
		t.Errorf("the UUID converted to %s", got)
	}
}

// TestWideIntegersSurviveConversion — the values GOL-653 is about must cross
// the boundary exactly, with no float in the middle.
func TestWideIntegersSurviveConversion(t *testing.T) {
	for _, want := range []int64{-9223372036854775808, 9223372036854775807, 9007199254740993} {
		var b valueBuilder
		root := b.push(types.MakeSchemaValueNodeS64Value(want))
		v, err := ValueToCore(b.tree(root))
		if err != nil {
			t.Fatalf("ValueToCore: %v", err)
		}
		got, ok := v.(core.S64Value)
		if !ok || got.Value != want {
			t.Errorf("s64 %d converted to %#v", want, v)
		}
	}
	var b valueBuilder
	root := b.push(types.MakeSchemaValueNodeU64Value(18446744073709551615))
	v, _ := ValueToCore(b.tree(root))
	if got, ok := v.(core.U64Value); !ok || got.Value != 18446744073709551615 {
		t.Errorf("u64 converted to %#v", v)
	}
}

func TestValueConversionRejectsAMalformedTree(t *testing.T) {
	var b valueBuilder
	root := b.push(types.MakeSchemaValueNodeListValue([]int32{42})) // no such node
	if _, err := ValueToCore(b.tree(root)); err == nil {
		t.Error("a tree referring to a missing node converted")
	}
}

// TestHandlesAreRefusedOffTarget — a capability only exists inside a
// component, so converting one natively must say so rather than invent a
// handle.
func TestHandlesAreRefusedOffTarget(t *testing.T) {
	if _, err := handleToWit(core.SecretValue{}); err == nil {
		t.Error("a secret handle converted off-target")
	}
}

// TestFailedFlatteningReleasesHandles — a value is built in one pass, so if it
// fails part-way the handles already taken have no other owner.
func TestFailedFlatteningReleasesHandles(t *testing.T) {
	taken := &countingHandle{}
	// The secret converts (off-target it fails), and whatever happens the
	// handle must not be left held.
	v := core.RecordValue{Fields: []core.SchemaValue{
		core.SecretValue{Handle: taken},
	}}
	if _, err := ValueToWit(v); err == nil {
		t.Fatal("flattening a foreign handle succeeded")
	}
	if taken.released == 0 {
		t.Error("a handle was left held after the conversion failed")
	}
}

type countingHandle struct{ released int }

func (h *countingHandle) Release() { h.released++ }

func TestEveryWitValueTagConverts(t *testing.T) {
	// If this fails, the bindings gained or lost a case: handle it in node()
	// and move the count deliberately.
	const declared = witValueTagCount
	if types.SchemaValueNodeStreamValue != declared-1 {
		t.Fatalf("the bindings declare value tags up to %d, but the converter pins %d; "+
			"a WIT case was added or removed", types.SchemaValueNodeStreamValue, declared-1)
	}
	for tag := uint8(0); tag < declared; tag++ {
		var b valueBuilder
		leaf := b.push(types.MakeSchemaValueNodeBoolValue(true))
		root := b.push(sampleValue(t, tag, leaf))
		_, err := ValueToCore(b.tree(root))
		// The host handles are refused off target, from their own case.
		if err != nil && strings.Contains(err.Error(), "unknown value node") {
			t.Errorf("tag %d fell through to the fallback: %v", tag, err)
		}
	}
}

// sampleValue builds a minimal node for a tag, using leaf wherever a nested
// value is needed.
func sampleValue(t *testing.T, tag uint8, leaf int32) types.SchemaValueNode {
	switch tag {
	case types.SchemaValueNodeBoolValue:
		return types.MakeSchemaValueNodeBoolValue(true)
	case types.SchemaValueNodeS8Value:
		return types.MakeSchemaValueNodeS8Value(1)
	case types.SchemaValueNodeS16Value:
		return types.MakeSchemaValueNodeS16Value(1)
	case types.SchemaValueNodeS32Value:
		return types.MakeSchemaValueNodeS32Value(1)
	case types.SchemaValueNodeS64Value:
		return types.MakeSchemaValueNodeS64Value(1)
	case types.SchemaValueNodeU8Value:
		return types.MakeSchemaValueNodeU8Value(1)
	case types.SchemaValueNodeU16Value:
		return types.MakeSchemaValueNodeU16Value(1)
	case types.SchemaValueNodeU32Value:
		return types.MakeSchemaValueNodeU32Value(1)
	case types.SchemaValueNodeU64Value:
		return types.MakeSchemaValueNodeU64Value(1)
	case types.SchemaValueNodeF32Value:
		return types.MakeSchemaValueNodeF32Value(1)
	case types.SchemaValueNodeF64Value:
		return types.MakeSchemaValueNodeF64Value(1)
	case types.SchemaValueNodeCharValue:
		return types.MakeSchemaValueNodeCharValue('a')
	case types.SchemaValueNodeStringValue:
		return types.MakeSchemaValueNodeStringValue("s")
	case types.SchemaValueNodeRecordValue:
		return types.MakeSchemaValueNodeRecordValue([]int32{leaf})
	case types.SchemaValueNodeVariantValue:
		return types.MakeSchemaValueNodeVariantValue(types.VariantValuePayload{Payload: witTypes.Some(leaf)})
	case types.SchemaValueNodeEnumValue:
		return types.MakeSchemaValueNodeEnumValue(0)
	case types.SchemaValueNodeFlagsValue:
		return types.MakeSchemaValueNodeFlagsValue([]bool{true})
	case types.SchemaValueNodeTupleValue:
		return types.MakeSchemaValueNodeTupleValue([]int32{leaf})
	case types.SchemaValueNodeListValue:
		return types.MakeSchemaValueNodeListValue([]int32{leaf})
	case types.SchemaValueNodeFixedListValue:
		return types.MakeSchemaValueNodeFixedListValue([]int32{leaf})
	case types.SchemaValueNodeMapValue:
		return types.MakeSchemaValueNodeMapValue([]types.MapEntry{{Key: leaf, Value: leaf}})
	case types.SchemaValueNodeOptionValue:
		return types.MakeSchemaValueNodeOptionValue(witTypes.Some(leaf))
	case types.SchemaValueNodeResultValue:
		return types.MakeSchemaValueNodeResultValue(types.MakeResultValuePayloadOkValue(witTypes.Some(leaf)))
	case types.SchemaValueNodeTextValue:
		return types.MakeSchemaValueNodeTextValue(types.TextValuePayload{Text: "t", Language: witTypes.None[string]()})
	case types.SchemaValueNodeBinaryValue:
		return types.MakeSchemaValueNodeBinaryValue(types.BinaryValuePayload{Bytes: []uint8{1}, MimeType: witTypes.None[string]()})
	case types.SchemaValueNodePathValue:
		return types.MakeSchemaValueNodePathValue("/p")
	case types.SchemaValueNodeUrlValue:
		return types.MakeSchemaValueNodeUrlValue("https://example.com")
	case types.SchemaValueNodeUuidValue:
		return types.MakeSchemaValueNodeUuidValue(types.Uuid{HighBits: 1, LowBits: 2})
	case types.SchemaValueNodeDatetimeValue:
		return types.MakeSchemaValueNodeDatetimeValue(types.Datetime{Seconds: 1})
	case types.SchemaValueNodeDurationValue:
		return types.MakeSchemaValueNodeDurationValue(types.DurationValuePayload{Nanoseconds: 1})
	case types.SchemaValueNodeQuantityValueNode:
		return types.MakeSchemaValueNodeQuantityValueNode(types.QuantityValue{Mantissa: 1, Unit: "kg"})
	case types.SchemaValueNodeUnionValue:
		return types.MakeSchemaValueNodeUnionValue(types.UnionValuePayload{Tag: "b", Body: leaf})
	case types.SchemaValueNodeSecretValue:
		return types.MakeSchemaValueNodeSecretValue(nil)
	case types.SchemaValueNodeQuotaTokenHandle:
		return types.MakeSchemaValueNodeQuotaTokenHandle(nil)
	case types.SchemaValueNodePermissionCardHandle:
		return types.MakeSchemaValueNodePermissionCardHandle(nil)
	case types.SchemaValueNodeStreamValue:
		return types.MakeSchemaValueNodeStreamValue(nil)
	}
	t.Fatalf("no sample for value tag %d", tag)
	return types.SchemaValueNode{}
}
