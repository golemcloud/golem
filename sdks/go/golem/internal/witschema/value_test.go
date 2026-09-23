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
	root := b.push(types.MakeSchemaValueNodeRecordValue([]int32{name, count, inner, items, amap, res}))

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
