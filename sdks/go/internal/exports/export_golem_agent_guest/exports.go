// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Package export_golem_agent_guest holds the hand-written export slots for
// golem:agent/guest@2.0.0.
//
// These are NOT generated: `componentize-go bindings` is run with
// --export-pkg-name pointing here, so the generated wasmexport glue in
// internal/wit/wit_exports calls into this package while never overwriting it.
// The SDK fills the slots from its init(); leaving one nil is a programming
// error and panics with a clear message rather than a nil-deref.
//
// initialize/invoke are `async func` in WIT, but the async lift is handled
// entirely by the generated glue (witAsync.Run), so these are plain blocking
// Go functions.
package export_golem_agent_guest

import (
	common "github.com/golemcloud/golem/sdks/go/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Exports is the set of slots the SDK must fill before the component is invoked.
var Exports struct {
	Initialize         func(agentType string, input types.SchemaValueTree, principal common.Principal) witTypes.Result[witTypes.Unit, common.AgentError]
	Invoke             func(methodName string, input types.SchemaValueTree, principal common.Principal) witTypes.Result[witTypes.Option[types.SchemaValueTree], common.AgentError]
	GetDefinition      func() common.AgentType
	DiscoverAgentTypes func() witTypes.Result[[]common.AgentType, common.AgentError]
}

func mustBeSet(name string, set bool) {
	if !set {
		panic("golem: export " + name + " was not registered — is the SDK imported?")
	}
}

func Initialize(agentType string, input types.SchemaValueTree, principal common.Principal) witTypes.Result[witTypes.Unit, common.AgentError] {
	mustBeSet("initialize", Exports.Initialize != nil)
	return Exports.Initialize(agentType, input, principal)
}

func Invoke(methodName string, input types.SchemaValueTree, principal common.Principal) witTypes.Result[witTypes.Option[types.SchemaValueTree], common.AgentError] {
	mustBeSet("invoke", Exports.Invoke != nil)
	return Exports.Invoke(methodName, input, principal)
}

func GetDefinition() common.AgentType {
	mustBeSet("get-definition", Exports.GetDefinition != nil)
	return Exports.GetDefinition()
}

func DiscoverAgentTypes() witTypes.Result[[]common.AgentType, common.AgentError] {
	mustBeSet("discover-agent-types", Exports.DiscoverAgentTypes != nil)
	return Exports.DiscoverAgentTypes()
}
