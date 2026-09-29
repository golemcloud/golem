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

// Package oplog reads and searches agents' oplogs.
//
// The entries are the host's own types, as the Rust SDK exposes them: switch on
// Entry.Tag() and read a case's parameters with the accessor of the same name.
//
//	for e, err := range oplog.Get(id, 0) {
//	    if err != nil { return err }
//	    switch e.Tag() {
//	    case oplog.AgentInvocationStarted:
//	        p := e.AgentInvocationStarted()
//	        …
//	    }
//	}
package oplog

import (
	"encoding/binary"

	"github.com/golemcloud/golem/sdks/go/golem"
	oplogwit "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_oplog"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Entry is one oplog entry. Its cases are the constants of this package.
type Entry = oplogwit.PublicOplogEntry

// The parameters of the commonly inspected cases.
type (
	CreateParameters                  = oplogwit.CreateParameters
	AgentInvocationStartedParameters  = oplogwit.AgentInvocationStartedParameters
	AgentInvocationFinishedParameters = oplogwit.AgentInvocationFinishedParameters
	ErrorParameters                   = oplogwit.ErrorParameters
	LogParameters                     = oplogwit.LogParameters
	// Timestamp is the time of an entry: seconds and nanoseconds since the epoch.
	Timestamp = oplogwit.Datetime
)

// SearchHit is an entry [Search] found, with its index in the oplog.
type SearchHit struct {
	Index uint64
	Entry Entry
}

func agentIDToWit(id golem.AgentID) types.AgentId {
	u := id.ComponentID
	return types.AgentId{
		ComponentId: types.ComponentId{Uuid: types.Uuid{
			HighBits: binary.BigEndian.Uint64(u[0:8]),
			LowBits:  binary.BigEndian.Uint64(u[8:16]),
		}},
		AgentId: id.AgentID,
	}
}

func readErrorFromWit(e oplogwit.OplogReadError) error {
	if e.Tag() == oplogwit.OplogReadErrorPermissionDenied {
		return &golem.AgentOperationError{Kind: golem.AgentOperationPermissionDenied}
	}
	return &golem.AgentOperationError{Kind: golem.AgentOperationBackendError, Message: e.InternalError()}
}
