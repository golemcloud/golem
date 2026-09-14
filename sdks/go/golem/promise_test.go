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
	"bytes"
	"testing"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// TestPromisePayloadJSON — a non-[]byte payload round-trips as JSON.
func TestPromisePayloadJSON(t *testing.T) {
	type Decision struct {
		Approved bool
		By       string
	}
	want := Decision{Approved: true, By: "alice"}

	data := encodePromisePayload(want)
	if string(data) != `{"Approved":true,"By":"alice"}` {
		t.Fatalf("encoded JSON = %s", data)
	}
	if got := decodePromisePayload[Decision](data); got != want {
		t.Fatalf("round-trip = %+v, want %+v", got, want)
	}
}

// TestPromisePayloadRawBytes — a []byte payload passes through verbatim (NOT
// base64-wrapped as json.Marshal would), so non-JSON external payloads survive.
func TestPromisePayloadRawBytes(t *testing.T) {
	raw := []byte{0x00, 0x01, 0xff, 'h', 'i'}

	data := encodePromisePayload(raw)
	if !bytes.Equal(data, raw) {
		t.Fatalf("[]byte should pass through raw, got %v", data)
	}
	if got := decodePromisePayload[[]byte](data); !bytes.Equal(got, raw) {
		t.Fatalf("[]byte round-trip = %v, want %v", got, raw)
	}
}

// TestPromisePayloadNamedBytesUsesJSON — the pass-through is for []byte exactly;
// a named type with []byte underlying still goes through JSON.
func TestPromisePayloadNamedBytesUsesJSON(t *testing.T) {
	type Raw []byte
	if got := encodePromisePayload(Raw("hi")); string(got) != `"aGk="` {
		t.Fatalf("named []byte should JSON-encode (base64), got %s", got)
	}
}

// TestPromiseIDRoundTrip — PromiseID survives conversion to the host wire form
// and back, and String() renders the component/agent/oplog form.
func TestPromiseIDRoundTrip(t *testing.T) {
	id := PromiseID{
		ComponentID: UUID{0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15},
		AgentID:     "Approval(\"order-7\")",
		OplogIndex:  42,
	}

	got := promiseIDFromWit(id.toWit())
	if got != id {
		t.Fatalf("round-trip = %+v, want %+v", got, id)
	}

	want := "00010203-0405-0607-0809-0a0b0c0d0e0f/Approval(\"order-7\")/42"
	if id.String() != want {
		t.Fatalf("String() = %q, want %q", id.String(), want)
	}
}

// TestPromiseIDFromWit — decoding a host PromiseId maps every nested field.
func TestPromiseIDFromWit(t *testing.T) {
	w := types.PromiseId{
		AgentId: types.AgentId{
			ComponentId: types.ComponentId{Uuid: types.Uuid{HighBits: 0x0001020304050607, LowBits: 0x08090a0b0c0d0e0f}},
			AgentId:     "Shop(\"acme\")",
		},
		OplogIdx: 99,
	}

	id := promiseIDFromWit(w)
	if id.AgentID != "Shop(\"acme\")" || id.OplogIndex != 99 {
		t.Fatalf("agent/oplog not mapped: %+v", id)
	}
	if id.ComponentID != (UUID{0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15}) {
		t.Fatalf("component uuid not mapped: %v", id.ComponentID)
	}
}
