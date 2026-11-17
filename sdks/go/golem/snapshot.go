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

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	"github.com/golemcloud/golem/sdks/go/golem/internal/witschema"
)

// Snapshotting: the host can ask the guest to serialize the running agent's
// state (save-snapshot) and later restore it (load-snapshot) — both are guest
// exports, wired in guest.go.
//
// Three modes, chosen per instance:
//   - if the state implements [MultipartSnapshotter], its JSON state and named
//     parts are framed as multipart/mixed;
//   - if the state implements [Snapshotter], its Save/Load bytes are used verbatim
//     (opaque payload);
//   - otherwise the state's exported fields are JSON-encoded. Unexported fields
//     are invisible to reflection, so a state with private fields must implement
//     Snapshotter to be captured — see [Snapshotter].
const (
	snapshotRawMIME  = "application/octet-stream"
	snapshotJSONMIME = "application/json"
)

// The agent's principal and constructor parameters travel with its state, since
// a restore has no initialize call to supply them and the agent ID does not
// encode the parameters. The constructor parameters are their schema-native
// wire form. A JSON snapshot is
// {"version":1,"principal":…,"constructor":…,"state":…}; a raw one is a version
// byte (2), the principal's length as a big-endian uint32, the principal, the
// constructor's length as a big-endian uint32, the constructor, then the
// Snapshotter's bytes.
const (
	snapshotJSONVersion = 1
	snapshotRawVersion  = 2
)

type jsonSnapshot struct {
	Version     int             `json:"version"`
	Principal   json.RawMessage `json:"principal"`
	Constructor json.RawMessage `json:"constructor"`
	State       json.RawMessage `json:"state"`
}

// saveState serializes an agent instance's state, principal and constructor
// parameters into a host snapshot. constructor is the parameters' wire JSON.
func saveState(state any, principal Principal, constructor json.RawMessage) (host.Snapshot, error) {
	p, err := marshalPrincipal(principal)
	if err != nil {
		return host.Snapshot{}, err
	}
	if !json.Valid(constructor) {
		return host.Snapshot{}, errors.New("golem: the snapshot's constructor parameters are not valid JSON")
	}
	if mp, ok := state.(MultipartSnapshotter); ok {
		parts, err := mp.SaveParts()
		if err != nil {
			return host.Snapshot{}, err
		}
		payload, mime, err := encodeMultipart(parts, p, constructor)
		if err != nil {
			return host.Snapshot{}, err
		}
		return host.Snapshot{Payload: payload, MimeType: mime}, nil
	}
	if sn, ok := state.(Snapshotter); ok {
		payload, err := sn.Save()
		if err != nil {
			return host.Snapshot{}, err
		}
		out := make([]byte, 0, 9+len(p)+len(constructor)+len(payload))
		out = append(out, snapshotRawVersion)
		out = binary.BigEndian.AppendUint32(out, uint32(len(p)))
		out = append(out, p...)
		out = binary.BigEndian.AppendUint32(out, uint32(len(constructor)))
		out = append(out, constructor...)
		out = append(out, payload...)
		return host.Snapshot{Payload: out, MimeType: snapshotRawMIME}, nil
	}
	s, err := json.Marshal(state)
	if err != nil {
		return host.Snapshot{}, err
	}
	payload, err := json.Marshal(jsonSnapshot{Version: snapshotJSONVersion, Principal: p, Constructor: constructor, State: s})
	if err != nil {
		return host.Snapshot{}, err
	}
	return host.Snapshot{Payload: payload, MimeType: snapshotJSONMIME}, nil
}

// snapshotConstructor renders constructor parameters in the wire form that a
// snapshot keeps. A capability — a secret, a quota token, a permission card or
// a stream — is valid only in the invocation that received it, so parameters
// that hold one cannot be kept.
func snapshotConstructor(tree types.SchemaValueTree) (json.RawMessage, error) {
	for _, n := range tree.ValueNodes {
		switch n.Tag() {
		case types.SchemaValueNodeSecretValue, types.SchemaValueNodeQuotaTokenHandle,
			types.SchemaValueNodePermissionCardHandle, types.SchemaValueNodeStreamValue:
			return nil, errors.New("golem: constructor parameters that hold a capability cannot be kept in a snapshot")
		}
	}
	v, err := witschema.ValueToCore(tree)
	if err != nil {
		return nil, err
	}
	return core.MarshalWireValue(v)
}

// restoredConstructor reads constructor parameters that [snapshotConstructor]
// rendered.
func restoredConstructor(data json.RawMessage) (types.SchemaValueTree, error) {
	v, err := core.UnmarshalWireValue(data)
	if err != nil {
		return types.SchemaValueTree{}, fmt.Errorf("golem: the snapshot's constructor parameters: %w", err)
	}
	return witschema.ValueToWit(v)
}

// savedState is a snapshot without its principal and constructor: the payload [loadState]
// reads, or the decoded parts of a multipart snapshot.
type savedState struct {
	Payload   []byte
	multipart *MultipartSnapshot
}

// splitSnapshot separates a snapshot into the agent's principal, the wire JSON
// of its constructor parameters and the state [loadState] reads.
func splitSnapshot(snap host.Snapshot) (Principal, json.RawMessage, savedState, error) {
	if _, ok := extractBoundary(snap.MimeType); ok {
		principal, constructor, parts, err := decodeMultipart(snap.Payload, snap.MimeType)
		if err != nil {
			return nil, nil, savedState{}, err
		}
		p, err := unmarshalPrincipal(principal)
		return p, constructor, savedState{multipart: &parts}, err
	}
	if snap.MimeType == snapshotJSONMIME {
		var js jsonSnapshot
		if err := json.Unmarshal(snap.Payload, &js); err != nil {
			return nil, nil, savedState{}, fmt.Errorf("golem: decoding the snapshot: %w", err)
		}
		if js.Version != snapshotJSONVersion {
			return nil, nil, savedState{}, fmt.Errorf("golem: unsupported JSON snapshot version %d", js.Version)
		}
		if js.Constructor == nil {
			return nil, nil, savedState{}, errors.New("golem: the snapshot has no constructor parameters")
		}
		p, err := unmarshalPrincipal(js.Principal)
		return p, js.Constructor, savedState{Payload: js.State}, err
	}
	b := snap.Payload
	if len(b) < 1 || b[0] != snapshotRawVersion {
		return nil, nil, savedState{}, errors.New("golem: not a snapshot this SDK wrote")
	}
	b = b[1:]
	field := func(what string) ([]byte, error) {
		if len(b) < 4 {
			return nil, fmt.Errorf("golem: snapshot too short for its %s", what)
		}
		n := binary.BigEndian.Uint32(b[:4])
		if uint64(len(b)-4) < uint64(n) {
			return nil, fmt.Errorf("golem: snapshot too short for its %s", what)
		}
		out := b[4 : 4+n]
		b = b[4+n:]
		return out, nil
	}
	principal, err := field("principal")
	if err != nil {
		return nil, nil, savedState{}, err
	}
	constructor, err := field("constructor parameters")
	if err != nil {
		return nil, nil, savedState{}, err
	}
	p, err := unmarshalPrincipal(principal)
	return p, constructor, savedState{Payload: b}, err
}

// restoreState rebuilds an agent's state from a snapshot without running its
// constructor, as the other SDKs do: whatever the constructor did — a call to
// another agent, an HTTP request — is already in the history the snapshot
// replaces, and must not happen again. The state starts from its zero value,
// the snapshot is loaded into it, and the [AgentImpl.OnRestore] hook, if any,
// rebuilds what the snapshot does not carry.
func restoreState(e *agentEntry, idVal reflect.Value, principal Principal, snap savedState) (any, error) {
	state := e.zeroState()
	if err := loadState(state, snap); err != nil {
		return nil, err
	}
	if e.restore != nil {
		if err := e.restore(state, idVal, principal); err != nil {
			return nil, fmt.Errorf("restoring %s: %w", e.name, err)
		}
	}
	return state, nil
}

// loadState restores an agent instance's state from a host snapshot. state must
// be the same shape saveState produced the snapshot from (a pointer to the state
// struct), so the decode writes back through it.
func loadState(state any, snap savedState) error {
	mp, isMultipart := state.(MultipartSnapshotter)
	switch {
	case isMultipart && snap.multipart != nil:
		return mp.LoadParts(*snap.multipart)
	case isMultipart:
		return errors.New("golem: the state loads a multipart snapshot, and this one is not")
	case snap.multipart != nil:
		return errors.New("golem: a multipart snapshot needs a state that implements MultipartSnapshotter")
	}
	if sn, ok := state.(Snapshotter); ok {
		return sn.Load(snap.Payload)
	}
	return json.Unmarshal(snap.Payload, state)
}
