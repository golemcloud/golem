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

package bridge

import (
	"encoding/json"
	"fmt"

	"github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/core/values"
)

// The worker service's runtime REST API, as openapi/golem-service.yaml defines
// it. Two shapes cross it and they are not the same:
//
//   - method and constructor parameters and results are schema-native
//     SchemaValue nodes, the {kind, value} wire form;
//   - a configuration override is a NormalizedJsonValue, which is ordinary
//     canonical JSON. The server holds configuration as plain JSON and applies
//     the schema itself.
//
// Sending one where the other belongs is accepted by neither side.

// AgentID is the ID of an agent: an opaque UUID that Golem issues, whose text
// form is the UUID. See [values.AgentID].
type AgentID = values.AgentID

// ConfigEntry overrides one agent configuration value. Value is canonical JSON
// — a string, a number, a map — not a schema.SchemaValue.
type ConfigEntry struct {
	Path  []string
	Value any
}

// InvocationMode is how the server should run the call.
type InvocationMode string

const (
	// ModeAwait runs the method and returns its result.
	ModeAwait InvocationMode = "await"
	// ModeSchedule enqueues the method. With no time it runs as soon as it
	// can, which is what fire-and-forget means here.
	ModeSchedule InvocationMode = "schedule"
)

type createAgentRequest struct {
	Parameters json.RawMessage   `json:"parameters"`
	Config     []configEntryDTO  `json:"config"`
	Env        map[string]string `json:"env"`
	Keyed      bool              `json:"keyed"`
}

// agentKeyRequest names the keyed agent of the constructor parameters.
type agentKeyRequest struct {
	Parameters json.RawMessage `json:"parameters"`
}

// invokeAgentRequest names the agent by exactly one of AgentID or Parameters.
type invokeAgentRequest struct {
	AgentID             *AgentID        `json:"agentId,omitempty"`
	Parameters          json.RawMessage `json:"parameters,omitempty"`
	Method              string          `json:"method"`
	MethodParameters    json.RawMessage `json:"methodParameters"`
	Mode                InvocationMode  `json:"mode"`
	ScheduleAt          *string         `json:"scheduleAt,omitempty"`
	ExpectedFingerprint string          `json:"expectedFingerprint,omitempty"`
}

type configEntryDTO struct {
	Path  []string `json:"path"`
	Value any      `json:"value"`
}

// agentReference is the server's AgentReference: an agent and the
// incarnation that a create, upsert or lookup reached.
type agentReference struct {
	AgentID          AgentID `json:"agentId"`
	AgentFingerprint string  `json:"agentFingerprint"`
}

// agentMetadata is the part of the server's AgentMetadataDto that GetByID
// checks.
type agentMetadata struct {
	AgentTypeName *string `json:"agentTypeName"`
	Fingerprint   string  `json:"fingerprint"`
}

// currentToken is the part of the registry's Token that names the account the
// token belongs to.
type currentToken struct {
	AccountID string `json:"accountId"`
}

// visibleEnvironments is the part of the registry's page of visible
// environments that names each environment's ID, names, and owning account.
type visibleEnvironments struct {
	Values []struct {
		Environment struct {
			ID   string `json:"id"`
			Name string `json:"name"`
		} `json:"environment"`
		Application struct {
			Name string `json:"name"`
		} `json:"application"`
		Account struct {
			ID string `json:"id"`
		} `json:"account"`
	} `json:"values"`
}

type invokeAgentResponse struct {
	AgentID           AgentID        `json:"agentId"`
	IdempotencyKey    string         `json:"idempotencyKey"`
	Result            *typedValueDTO `json:"result"`
	ComponentRevision *uint64        `json:"componentRevision"`
}

// typedValueDTO is the server's ExternalTypedSchemaValue. Only the value is
// read: a caller that generated its client from the same schema already knows
// the graph, and a caller that does not can read it with schema.UnmarshalWireGraph.
type typedValueDTO struct {
	Graph json.RawMessage `json:"graph"`
	Value json.RawMessage `json:"value"`
}

// Result is what an awaited invocation produced.
type Result struct {
	AgentID        AgentID
	IdempotencyKey string
	// Value is nil when the method returns nothing.
	Value schema.SchemaValue
	// Graph is the schema the value was built against, as it arrived. It is
	// left unparsed because a generated client already holds the schema; a
	// dynamic caller reads it with SchemaGraph.
	Graph json.RawMessage
}

// SchemaGraph parses the schema the result's value was built against.
func (r Result) SchemaGraph() (schema.SchemaGraph, error) {
	if len(r.Graph) == 0 {
		return schema.SchemaGraph{}, fmt.Errorf("golem: the result carried no schema")
	}
	return schema.UnmarshalWireGraph(r.Graph)
}

// Receipt is what an enqueued invocation produced. There is no result: the
// method has not run yet.
type Receipt struct {
	AgentID        AgentID
	IdempotencyKey string
}

func configEntriesToDTO(entries []ConfigEntry) []configEntryDTO {
	// The field is not optional on the wire, so an empty list must encode as
	// [] rather than null.
	out := make([]configEntryDTO, 0, len(entries))
	for _, entry := range entries {
		out = append(out, configEntryDTO(entry))
	}
	return out
}
