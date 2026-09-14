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

package keyvalue

import "testing"

// TestTypedCodecRoundTrip — the pure JSON codec behind Store round-trips a value.
// (The host-backed Get/Set are exercised by the playground e2e; here we cover the
// only pure logic — the typed codec.)
func TestTypedCodecRoundTrip(t *testing.T) {
	type Cart struct {
		Items []string
		Total int64
	}
	want := Cart{Items: []string{"a", "b"}, Total: 42}

	raw, err := marshalValue(want)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	if string(raw) != `{"Items":["a","b"],"Total":42}` {
		t.Fatalf("encoded = %s", raw)
	}
	got, err := unmarshalValue[Cart](raw)
	if err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if got.Total != want.Total || len(got.Items) != 2 {
		t.Fatalf("round-trip = %+v, want %+v", got, want)
	}
}

// TestErrorFormatting — the host-error wrapper renders its trace.
func TestErrorFormatting(t *testing.T) {
	e := &Error{Trace: "bucket not found"}
	if e.Error() != "golem/keyvalue: bucket not found" {
		t.Fatalf("Error() = %q", e.Error())
	}
}
