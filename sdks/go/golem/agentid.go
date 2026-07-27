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
	"fmt"
	"reflect"

	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// UUID is a 128-bit identifier, used here for the phantom id of an ephemeral
// agent instance.
type UUID [16]byte

// String renders the canonical 8-4-4-4-12 hex form.
func (u UUID) String() string {
	return fmt.Sprintf("%x-%x-%x-%x-%x", u[0:4], u[4:6], u[6:8], u[8:10], u[10:16])
}

func uuidFromWit(w types.Uuid) UUID {
	var u UUID
	binary.BigEndian.PutUint64(u[0:8], w.HighBits)
	binary.BigEndian.PutUint64(u[8:16], w.LowBits)
	return u
}

// ParsedAgentID is the typed decomposition of an agent id string: the agent
// type name, the constructor parameters decoded back into the Id type, and the
// phantom id (present for phantom/ephemeral instances).
type ParsedAgentID[Id any] struct {
	TypeName  string
	ID        Id
	PhantomID Option[UUID]
}

// ParseAgentID decodes an agent id string (e.g. from [Context.AgentID] or a
// method parameter carrying another agent's id) into its typed form.
//
// Id must be the target agent's Id struct — the same type used with
// [DefineAgent]/[ClientFor] — so the constructor parameters decode back into it.
// This calls the host `parse-agent-id`, so it runs only inside a component, not
// in native tests; the decode itself ([decodeAgentIDParams]) is separately
// testable.
func ParseAgentID[Id any](agentID string) (ParsedAgentID[Id], error) {
	res := host.ParseAgentId(agentID)
	if res.IsErr() {
		return ParsedAgentID[Id]{}, fmt.Errorf("golem: parsing agent id %q: %w", agentID, agentErrorToGo(res.Err()))
	}
	t := res.Ok()

	id, err := decodeAgentIDParams[Id](t.F1.Value)
	if err != nil {
		return ParsedAgentID[Id]{}, fmt.Errorf("golem: agent id %q: %w", agentID, err)
	}

	phantom := None[UUID]()
	if t.F2.IsSome() {
		phantom = Some(uuidFromWit(t.F2.Some()))
	}
	return ParsedAgentID[Id]{TypeName: t.F0, ID: id, PhantomID: phantom}, nil
}

// decodeAgentIDParams decodes a constructor-parameter value tree into the Id
// struct — the same positional record decode initialize uses for the raw
// constructor input. Pure (no host import), so it is natively testable.
func decodeAgentIDParams[Id any](value types.SchemaValueTree) (Id, error) {
	var zero Id
	idType := reflect.TypeFor[Id]()
	if idType.Kind() != reflect.Struct {
		return zero, fmt.Errorf("Id type %s must be a struct", idType)
	}
	idVal := reflect.New(idType).Elem()
	if err := decodeParams(value, structFields(idType), idVal); err != nil {
		return zero, err
	}
	return idVal.Interface().(Id), nil
}
