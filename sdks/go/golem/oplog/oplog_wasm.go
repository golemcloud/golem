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

//go:build wasip1

package oplog

import (
	"encoding/binary"
	"iter"

	"github.com/golemcloud/golem/sdks/go/golem"
	oplogwit "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_oplog"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Get reads an agent's oplog from the entry at index start. Pages are fetched
// as the loop advances; a failing page is yielded as an error and ends the
// iteration.
func Get(id golem.AgentID, start uint64) iter.Seq2[Entry, error] {
	return func(yield func(Entry, error) bool) {
		r := oplogwit.MakeGetOplog(agentIDToWit(id), start)
		defer r.Drop()
		for {
			res := r.GetNext()
			if res.IsErr() {
				yield(Entry{}, readErrorFromWit(res.Err()))
				return
			}
			page := res.Ok()
			if page.IsNone() {
				return
			}
			for _, e := range page.Some() {
				if !yield(e, nil) {
					return
				}
			}
		}
	}
}

// Search finds the entries of an agent's oplog that match text, in pages like
// [Get].
func Search(id golem.AgentID, text string) iter.Seq2[SearchHit, error] {
	return func(yield func(SearchHit, error) bool) {
		r := oplogwit.MakeSearchOplog(agentIDToWit(id), text)
		defer r.Drop()
		for {
			res := r.GetNext()
			if res.IsErr() {
				yield(SearchHit{}, readErrorFromWit(res.Err()))
				return
			}
			page := res.Ok()
			if page.IsNone() {
				return
			}
			for _, h := range page.Some() {
				if !yield(SearchHit{Index: h.F0, Entry: h.F1}, nil) {
					return
				}
			}
		}
	}
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
