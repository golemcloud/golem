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

package blobstore

import (
	"testing"
	"time"
)

// TestTypedCodecRoundTrip — the pure JSON codec behind Store round-trips a value.
func TestTypedCodecRoundTrip(t *testing.T) {
	type Doc struct {
		Title string
		Tags  []string
	}
	want := Doc{Title: "spec", Tags: []string{"a", "b"}}
	raw, err := marshalValue(want)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	got, err := unmarshalValue[Doc](raw)
	if err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if got.Title != want.Title || len(got.Tags) != 2 {
		t.Fatalf("round-trip = %+v", got)
	}
}

// TestMillis — host millis map to time.Time.
func TestMillis(t *testing.T) {
	if got := millis(1500); !got.Equal(time.UnixMilli(1500)) {
		t.Fatalf("millis(1500) = %v", got)
	}
}

// TestErrorFormatting — the bare-string host error renders with the package prefix.
func TestErrorFormatting(t *testing.T) {
	if e := bsError("no such container"); e.Error() != "golem/blobstore: no such container" {
		t.Fatalf("Error() = %q", e.Error())
	}
}
