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
	"encoding/json"

	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
)

// Snapshotting: the host can ask the guest to serialize the running agent's
// state (save-snapshot) and later restore it (load-snapshot) — both are guest
// exports, wired in guest.go.
//
// Two modes, chosen per instance:
//   - if the state implements [Snapshotter], its Save/Load bytes are used verbatim
//     (opaque payload);
//   - otherwise the state's exported fields are JSON-encoded. Unexported fields
//     are invisible to reflection, so a state with private fields must implement
//     Snapshotter to be captured — see [Snapshotter].
const (
	snapshotRawMIME  = "application/octet-stream"
	snapshotJSONMIME = "application/json"
)

// saveState serializes an agent instance's state into a host snapshot.
func saveState(state any) (host.Snapshot, error) {
	if sn, ok := state.(Snapshotter); ok {
		payload, err := sn.Save()
		if err != nil {
			return host.Snapshot{}, err
		}
		return host.Snapshot{Payload: payload, MimeType: snapshotRawMIME}, nil
	}
	payload, err := json.Marshal(state)
	if err != nil {
		return host.Snapshot{}, err
	}
	return host.Snapshot{Payload: payload, MimeType: snapshotJSONMIME}, nil
}

// loadState restores an agent instance's state from a host snapshot. state must
// be the same shape saveState produced the snapshot from (a pointer to the state
// struct), so the decode writes back through it.
func loadState(state any, snap host.Snapshot) error {
	if sn, ok := state.(Snapshotter); ok {
		return sn.Load(snap.Payload)
	}
	return json.Unmarshal(snap.Payload, state)
}
