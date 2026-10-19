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

package schema

import "testing"

// fakeHandle records that it was given back.
type fakeHandle struct{ released int }

func (h *fakeHandle) Release() { h.released++ }

// TestReleaseAllReachesNestedHandles — the point of the walker: a value that
// fails part-way through being built must give back every handle it already
// took, wherever in the structure it sits.
func TestReleaseAllReachesNestedHandles(t *testing.T) {
	a, b, c, d := &fakeHandle{}, &fakeHandle{}, &fakeHandle{}, &fakeHandle{}

	inner := SchemaValue(StreamValue{Handle: c})
	value := RecordValue{Fields: []SchemaValue{
		SecretValue{Handle: a},
		ListValue{Items: []SchemaValue{QuotaTokenValue{Handle: b}}},
		OptionValue{Value: &inner},
		MapValue{Entries: []MapEntry{{
			Key:   StringValue{Value: "k"},
			Value: PermissionCardValue{Handle: d},
		}}},
	}}

	ReleaseAll(value)
	for i, h := range []*fakeHandle{a, b, c, d} {
		if h.released != 1 {
			t.Errorf("handle %d released %d times, want 1", i, h.released)
		}
	}
}

// TestReleaseAllIgnoresPlainData — releasing a value that owns nothing must be
// harmless, since the caller cannot know in advance whether it does.
func TestReleaseAllIgnoresPlainData(t *testing.T) {
	ReleaseAll(RecordValue{Fields: []SchemaValue{
		StringValue{Value: "x"},
		S64Value{Value: 1},
		TupleValue{Elements: []SchemaValue{BoolValue{Value: true}}},
	}})
}

// TestReleaseAllToleratesAbsentHandles — a partially built value can hold a
// case whose handle was never taken.
func TestReleaseAllToleratesAbsentHandles(t *testing.T) {
	ReleaseAll(RecordValue{Fields: []SchemaValue{
		SecretValue{},
		OptionValue{},
		ResultValue{},
		VariantValue{},
	}})
}
