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
	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

var hostOps agentHost = witHost{}

type witHost struct{}

func (witHost) getOplogIndex() uint64      { return apiHost.GetOplogIndex() }
func (witHost) setOplogIndex(index uint64) { apiHost.SetOplogIndex(index) }

func (witHost) fork() (bool, UUID, error) {
	res := apiHost.Fork()
	if res.IsErr() {
		return false, UUID{}, agentOperationErrorFromWit(res.Err())
	}
	r := res.Ok()
	if r.Tag() == apiHost.ForkResultForked {
		return true, uuidFromWit(r.Forked().ForkedPhantomId), nil
	}
	return false, uuidFromWit(r.Original().ForkedPhantomId), nil
}

func (witHost) getSelfMetadata() (apiHost.AgentMetadata, error) {
	res := apiHost.GetSelfMetadata()
	if res.IsErr() {
		return apiHost.AgentMetadata{}, agentOperationErrorFromWit(res.Err())
	}
	return res.Ok(), nil
}

func (witHost) getAgentMetadata(id types.AgentId) (apiHost.AgentMetadata, bool) {
	o := apiHost.GetAgentMetadata(id)
	if o.IsNone() {
		return apiHost.AgentMetadata{}, false
	}
	return o.Some(), true
}

func (witHost) getAgents(component types.ComponentId, filter witTypes.Option[apiHost.AgentAnyFilter], precise bool) agentPages {
	return witAgentPages{apiHost.MakeGetAgents(component, filter, precise)}
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

func (witHost) forkAgent(source, target types.AgentId, cutOff uint64) error {
	return unitResult(apiHost.ForkAgent(source, target, cutOff))
}

func (witHost) resolveComponentID(reference string) (types.ComponentId, bool) {
	o := apiHost.ResolveComponentId(reference)
	if o.IsNone() {
		return types.ComponentId{}, false
	}
	return o.Some(), true
}

func (witHost) resolveAgentID(reference, agentName string, strict bool) (types.AgentId, bool) {
	o := apiHost.ResolveAgentId(reference, agentName)
	if strict {
		o = apiHost.ResolveAgentIdStrict(reference, agentName)
	}
	if o.IsNone() {
		return types.AgentId{}, false
	}
	return o.Some(), true
}
