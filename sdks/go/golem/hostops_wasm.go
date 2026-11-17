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

package golem

import (
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	invocationContext "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_invocation_context"
	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	environment "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_cli_0_3_0_environment"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

var hostOps agentHost = witHost{}

// selfEnv reads an environment variable through the durable host environment
// on each call: replay sees the value the call saw when it ran, and a copy made
// by a fork sees its own agent ID. os.Getenv would return the value captured at
// start-up. The host shows a variable that Golem sets only when the agent's
// permission card allows the agent to read it; otherwise the result is "".
var selfEnv = func(name string) string {
	for _, kv := range environment.GetEnvironment() {
		if kv.F0 == name {
			return kv.F1
		}
	}
	return ""
}

// immediateCaller reads the principal that made the running invocation.
var immediateCaller = func() common.Principal { return invocationContext.Caller().Immediate }

type witHost struct{}

func (witHost) fork() (bool, AgentID, error) {
	res := apiHost.Fork()
	if res.IsErr() {
		return false, AgentID{}, agentOperationErrorFromWit(res.Err())
	}
	r := res.Ok()
	if r.Tag() == apiHost.ForkResultForked {
		return true, agentIDFromWit(r.Forked().ForkedAgentId), nil
	}
	return false, agentIDFromWit(r.Original().ForkedAgentId), nil
}

func (witHost) getSelfMetadata() (apiHost.AgentMetadata, error) {
	return selfMetadataResult(apiHost.GetSelfMetadata())
}

func (witHost) getAgentMetadata(id types.AgentId) (apiHost.AgentMetadata, bool) {
	o := apiHost.GetAgentMetadata(id)
	if o.IsNone() {
		return apiHost.AgentMetadata{}, false
	}
	return o.Some(), true
}

func (witHost) getAgents(agentTypeName string, filter witTypes.Option[apiHost.AgentAnyFilter], precise bool) agentPages {
	return witAgentPages{apiHost.MakeGetAgents(agentTypeName, filter, precise)}
}

type witAgentPages struct{ r *apiHost.GetAgents }

func (p witAgentPages) next() ([]apiHost.AgentMetadata, bool, error) {
	res := p.r.GetNext()
	if res.IsErr() {
		return nil, false, agentOperationErrorFromWit(res.Err())
	}
	page := res.Ok()
	if page.IsNone() {
		return nil, false, nil
	}
	return page.Some(), true, nil
}

func (p witAgentPages) close() { p.r.Drop() }

func unitResult(res witTypes.Result[witTypes.Unit, apiHost.AgentOperationError]) error {
	if res.IsErr() {
		return agentOperationErrorFromWit(res.Err())
	}
	return nil
}

func (witHost) updateAgent(id types.AgentId, revision uint64, mode uint8) error {
	return unitResult(apiHost.UpdateAgent(id, revision, mode))
}

func (witHost) revertAgent(id types.AgentId, target apiHost.RevertAgentTarget) error {
	return unitResult(apiHost.RevertAgent(id, target))
}

func (witHost) forkAgent(source types.AgentId, cutOff uint64) (types.AgentId, error) {
	res := apiHost.ForkAgent(source, cutOff)
	if res.IsErr() {
		return types.AgentId{}, agentOperationErrorFromWit(res.Err())
	}
	return res.Ok(), nil
}

func (witHost) resolveComponentID(reference string) (types.ComponentId, bool) {
	o := apiHost.ResolveComponentId(reference)
	if o.IsNone() {
		return types.ComponentId{}, false
	}
	return o.Some(), true
}
