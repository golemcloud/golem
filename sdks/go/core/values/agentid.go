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

package values

// AgentID is the ID of a Golem agent: an opaque UUID that Golem issues. It
// names an agent within one environment and holds nothing else; the agent's
// type and constructor parameters cannot be read out of it.
//
// Its text form, from String and MarshalText, is the UUID. The zero AgentID
// names no agent.
type AgentID struct{ uuid UUID }

// AgentIDFromUUID is the agent ID whose UUID is u.
func AgentIDFromUUID(u UUID) AgentID { return AgentID{uuid: u} }

// UUID returns the agent ID's UUID.
func (id AgentID) UUID() UUID { return id.uuid }

// IsZero reports whether id is the zero AgentID.
func (id AgentID) IsZero() bool { return id.uuid == UUID{} }

// String renders the UUID in its canonical lowercase form.
func (id AgentID) String() string { return id.uuid.String() }

// MarshalText renders the UUID, so an AgentID is a JSON string.
func (id AgentID) MarshalText() ([]byte, error) { return []byte(id.uuid.String()), nil }

// UnmarshalText reads the hyphenated UUID form.
func (id *AgentID) UnmarshalText(text []byte) error {
	u, err := ParseUUID(string(text))
	if err != nil {
		return err
	}
	id.uuid = u
	return nil
}
