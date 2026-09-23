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
)

// The worker service's agent REST API, as openapi/golem-service.yaml defines
// it. Two shapes cross it and they are not the same:
//
//   - method parameters and results are schema-native SchemaValue nodes, the
//     {kind, value} wire form;
//   - a configuration override is a NormalizedJsonValue, which is ordinary
//     canonical JSON. The server holds configuration as plain JSON and applies
//     the schema itself.
//
// Sending one where the other belongs is accepted by neither side.

// AgentID identifies a resolved agent: the component it belongs to, and its id
// within that component.
type AgentID struct {
	ComponentID string `json:"componentId"`
	AgentID     string `json:"agentId"`
}

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
	AppName       string           `json:"appName"`
	EnvName       string           `json:"envName"`
	AgentTypeName string           `json:"agentTypeName"`
	Parameters    json.RawMessage  `json:"parameters"`
	PhantomID     *string          `json:"phantomId,omitempty"`
	Config        []configEntryDTO `json:"config"`
}

type invokeAgentRequest struct {
	AppName          string           `json:"appName"`
	EnvName          string           `json:"envName"`
	AgentTypeName    string           `json:"agentTypeName"`
	Parameters       json.RawMessage  `json:"parameters"`
	PhantomID        *string          `json:"phantomId,omitempty"`
	Config           []configEntryDTO `json:"config"`
	MethodName       string           `json:"methodName"`
	MethodParameters json.RawMessage  `json:"methodParameters"`
	Mode             InvocationMode   `json:"mode"`
	ScheduleAt       *string          `json:"scheduleAt,omitempty"`
	IdempotencyKey   *string          `json:"idempotencyKey,omitempty"`
}

type configEntryDTO struct {
	Path  []string `json:"path"`
	Value any      `json:"value"`
}

type createAgentResponse struct {
	AgentID           AgentID `json:"agentId"`
	ComponentRevision *uint64 `json:"componentRevision"`
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
