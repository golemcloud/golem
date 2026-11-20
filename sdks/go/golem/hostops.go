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
	"fmt"

	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// agentHost is the host API behind forking and agent management.
// The wasm build binds the generated host calls (hostops_wasm.go); a native
// build has no host, so tests install their own.
type agentHost interface {
	fork() (forked bool, forkedID AgentID, err error)
	getSelfMetadata() (apiHost.AgentMetadata, error)
	getAgentMetadata(id types.AgentId) (apiHost.AgentMetadata, bool)
	getAgents(agentTypeName string, filter witTypes.Option[apiHost.AgentAnyFilter], precise bool) agentPages
	updateAgent(id types.AgentId, revision uint64, mode uint8) error
	revertAgent(id types.AgentId, target apiHost.RevertAgentTarget) error
	forkAgent(source types.AgentId, cutOff uint64) (types.AgentId, error)
	resolveComponentID(reference string) (types.ComponentId, bool)
}

// agentPages yields the pages of an agent listing.
type agentPages interface {
	next() (page []apiHost.AgentMetadata, more bool, err error)
	close()
}

// agentIDEnvVar is how the executor tells the guest which agent instance it
// is, as the agent ID's UUID text; agentTypeEnvVar names its agent type.
const (
	agentIDEnvVar   = "GOLEM_AGENT_ID"
	agentTypeEnvVar = "GOLEM_AGENT_TYPE"
)

// envVarNotReadable is the error for an agent that cannot read a variable that
// Golem sets. The executor shows such a variable only when the agent's
// permission card allows the agent to read it.
func envVarNotReadable(name string) string {
	return name + " is not readable: the agent's permission card must allow env(?agent) @ ?agent : read : " + name
}

// selfAgentID reads the running agent's ID. It is the zero AgentID when the
// agent cannot read GOLEM_AGENT_ID. A malformed value is an SDK invariant and
// panics.
func selfAgentID() AgentID {
	text := selfEnv(agentIDEnvVar)
	if text == "" {
		return AgentID{}
	}
	u, err := ParseUUID(text)
	if err != nil {
		panic(fmt.Errorf("golem: %s is %q, not an agent ID: %w", agentIDEnvVar, text, err))
	}
	return AgentIDFromUUID(u)
}
