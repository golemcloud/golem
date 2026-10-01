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
	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// agentHost is the host API behind checkpoints, forking and agent management.
// The wasm build binds the generated host calls (hostops_wasm.go); a native
// build has no host, so tests install their own.
type agentHost interface {
	getOplogIndex() uint64
	setOplogIndex(index uint64)
	fork() (forked bool, phantomID UUID, err error)
	getSelfMetadata() (apiHost.AgentMetadata, error)
	getAgentMetadata(id types.AgentId) (apiHost.AgentMetadata, bool)
	getAgents(component types.ComponentId, filter witTypes.Option[apiHost.AgentAnyFilter], precise bool) agentPages
	updateAgent(id types.AgentId, revision uint64, mode uint8) error
	revertAgent(id types.AgentId, target apiHost.RevertAgentTarget) error
	forkAgent(source, target types.AgentId, cutOff uint64) error
	resolveComponentID(reference string) (types.ComponentId, bool)
	resolveAgentID(reference, agentName string, strict bool) (types.AgentId, bool)
}

// agentPages yields the pages of an agent listing.
type agentPages interface {
	next() (page []apiHost.AgentMetadata, more bool, err error)
	close()
}
