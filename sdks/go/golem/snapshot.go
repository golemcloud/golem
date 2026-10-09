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
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"reflect"

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

// The agent's principal travels with its state, since a restore has no
// initialize call to supply it. The layout is the
// Rust SDK's: a JSON snapshot is {"version":1,"principal":…,"state":…}; a raw one
// is a version byte (2), the principal's length as a big-endian uint32, the
// principal, then the Snapshotter's bytes.
const (
	snapshotJSONVersion = 1
	snapshotRawVersion  = 2
)

type jsonSnapshot struct {
	Version   int             `json:"version"`
	Principal json.RawMessage `json:"principal"`
	State     json.RawMessage `json:"state"`
}

// saveState serializes an agent instance's state and principal into a host
// snapshot.
func saveState(state any, principal Principal) (host.Snapshot, error) {
	p, err := marshalPrincipal(principal)
	if err != nil {
		return host.Snapshot{}, err
	}
	if sn, ok := state.(Snapshotter); ok {
		payload, err := sn.Save()
		if err != nil {
			return host.Snapshot{}, err
		}
		out := make([]byte, 0, 5+len(p)+len(payload))
		out = append(out, snapshotRawVersion)
		out = binary.BigEndian.AppendUint32(out, uint32(len(p)))
		out = append(out, p...)
		out = append(out, payload...)
		return host.Snapshot{Payload: out, MimeType: snapshotRawMIME}, nil
	}
	s, err := json.Marshal(state)
	if err != nil {
		return host.Snapshot{}, err
	}
	payload, err := json.Marshal(jsonSnapshot{Version: snapshotJSONVersion, Principal: p, State: s})
	if err != nil {
		return host.Snapshot{}, err
	}
	return host.Snapshot{Payload: payload, MimeType: snapshotJSONMIME}, nil
}

// splitSnapshot separates a snapshot into the agent's principal and the state
// payload [loadState] reads.
func splitSnapshot(snap host.Snapshot) (Principal, host.Snapshot, error) {
	if snap.MimeType == snapshotJSONMIME {
		var js jsonSnapshot
		if err := json.Unmarshal(snap.Payload, &js); err != nil {
			return nil, snap, fmt.Errorf("golem: decoding the snapshot: %w", err)
		}
		if js.Version != snapshotJSONVersion {
			return nil, snap, fmt.Errorf("golem: unsupported JSON snapshot version %d", js.Version)
		}
		p, err := unmarshalPrincipal(js.Principal)
		return p, host.Snapshot{Payload: js.State, MimeType: snap.MimeType}, err
	}
	b := snap.Payload
	if len(b) < 5 || b[0] != snapshotRawVersion {
		return nil, snap, errors.New("golem: not a snapshot this SDK wrote")
	}
	n := binary.BigEndian.Uint32(b[1:5])
	if uint64(len(b)-5) < uint64(n) {
		return nil, snap, errors.New("golem: snapshot too short for its principal")
	}
	p, err := unmarshalPrincipal(b[5 : 5+n])
	return p, host.Snapshot{Payload: b[5+n:], MimeType: snap.MimeType}, err
}

// restoreState rebuilds an agent's state from a snapshot without running its
// constructor, as the other SDKs do: whatever the constructor did — a call to
// another agent, an HTTP request — is already in the history the snapshot
// replaces, and must not happen again. The state starts from its zero value,
// the snapshot is loaded into it, and the [AgentImpl.OnRestore] hook, if any,
// rebuilds what the snapshot does not carry.
func restoreState(e *agentEntry, idVal reflect.Value, agentID string, principal Principal, snap host.Snapshot) (any, error) {
	state := e.zeroState()
	if err := loadState(state, snap); err != nil {
		return nil, err
	}
	if e.restore != nil {
		if err := e.restore(state, idVal, agentID, principal); err != nil {
			return nil, fmt.Errorf("restoring %s: %w", e.name, err)
		}
	}
	return state, nil
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
