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
	"github.com/golemcloud/golem/sdks/go/core/values"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// UUID is a 128-bit identifier, lowering to the WIT uuid type. See
// [values.UUID].
type UUID = values.UUID

// ParseUUID reads the 8-4-4-4-12 hexadecimal form of a UUID.
func ParseUUID(s string) (UUID, error) { return values.ParseUUID(s) }

func uuidFromWit(w types.Uuid) UUID { return engine.UUIDFromWit(w) }

func uuidToWit(u UUID) types.Uuid { return engine.UUIDToWit(u) }

// AgentID is the ID of an agent: an opaque UUID that Golem issues, whose text
// form is the UUID. It names an agent within one environment. The agent's type
// and constructor parameters cannot be read out of it: address an agent by its
// parameters with Upsert, and by its ID with GetByID. See [values.AgentID].
type AgentID = values.AgentID

// AgentIDFromUUID is the agent ID whose UUID is u.
func AgentIDFromUUID(u UUID) AgentID { return values.AgentIDFromUUID(u) }

func agentIDFromWit(w types.AgentId) AgentID { return AgentIDFromUUID(uuidFromWit(w.Uuid)) }

func agentIDToWit(id AgentID) types.AgentId { return types.AgentId{Uuid: uuidToWit(id.UUID())} }
