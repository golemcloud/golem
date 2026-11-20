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
	"testing"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

func TestUUIDFromWitAndString(t *testing.T) {
	// HighBits/LowBits laid out big-endian across the 16 bytes.
	u := uuidFromWit(types.Uuid{HighBits: 0x0011223344556677, LowBits: 0x8899aabbccddeeff})
	want := "00112233-4455-6677-8899-aabbccddeeff"
	if u.String() != want {
		t.Fatalf("uuid = %s, want %s", u.String(), want)
	}
}

func TestAgentIDWitRoundTripKeepsTheUUID(t *testing.T) {
	w := types.AgentId{Uuid: types.Uuid{HighBits: 0x0000000700008000, LowBits: 0x8000000000000004}}
	id := agentIDFromWit(w)
	if id.String() != "00000007-0000-8000-8000-000000000004" {
		t.Fatalf("agent id = %s", id)
	}
	if agentIDToWit(id) != w {
		t.Fatalf("round trip = %+v, want %+v", agentIDToWit(id), w)
	}
}
