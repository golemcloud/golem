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
	"fmt"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// DiscoverAgentTypes returns a snapshot of every agent type deployed in the
// caller's environment.
func DiscoverAgentTypes() []ReflectedAgentType {
	registered := host.GetAllAgentTypes()
	out := make([]ReflectedAgentType, 0, len(registered))
	for _, r := range registered {
		out = append(out, ReflectedAgentType{wit: r.AgentType})
	}
	return out
}

// DiscoverAgentType looks one agent type up by name. A missing deployment, or
// one the caller cannot see, reports false rather than failing.
func DiscoverAgentType(name string) (ReflectedAgentType, bool) {
	found := host.GetAgentType(name)
	if found.IsNone() {
		return ReflectedAgentType{}, false
	}
	return ReflectedAgentType{wit: found.Some().AgentType}, true
}

// DiscoverAgentTypeByID looks up the agent type behind an existing agent id.
// Discovery never creates the agent.
func DiscoverAgentTypeByID(agentID string) (ReflectedAgentType, bool) {
	found := host.GetAgentTypeByAgentId(agentID)
	if found.IsNone() {
		return ReflectedAgentType{}, false
	}
	return ReflectedAgentType{wit: found.Some().AgentType}, true
}

// witRPC invokes through the host's RPC resource.
type witRPC struct {
	rpc    *host.WasmRpc
	target string
}

func (w witRPC) invokeAndAwait(method string, input types.SchemaValueTree) (types.SchemaValueTree, bool, error) {
	res := w.rpc.InvokeAndAwait(method, input, noScopeCard())
	if res.Tag() == witTypes.ResultErr {
		return types.SchemaValueTree{}, false, rpcErrorToGo(w.target, method, res.Err())
	}
	out := res.Ok().Result
	if out.IsNone() {
		return types.SchemaValueTree{}, false, nil
	}
	return out.Some(), true, nil
}

// Bind connects to the agent this constructor value identifies, creating it if
// it does not exist yet, exactly as a typed client would.
func (r ReflectedAgentType) Bind(ctorArgs map[string]any, opts ...ClientOpt) (*ReflectedAgentClient, error) {
	return r.bind(ctorArgs, witTypes.None[types.Uuid](), opts)
}

// BindPhantom addresses a specific phantom instance of the agent.
func (r ReflectedAgentType) BindPhantom(ctorArgs map[string]any, phantom UUID, opts ...ClientOpt) (*ReflectedAgentClient, error) {
	return r.bind(ctorArgs, witTypes.Some(uuidToWit(phantom)), opts)
}

func (r ReflectedAgentType) bind(
	ctorArgs map[string]any, phantom witTypes.Option[types.Uuid], opts []ClientOpt,
) (*ReflectedAgentClient, error) {
	ctor, err := r.Constructor().PackJSON(ctorArgs)
	if err != nil {
		return nil, fmt.Errorf("golem: %s constructor: %w", r.Name(), err)
	}

	var o clientOpts
	for _, opt := range opts {
		opt(&o)
	}
	agentConfig, cfgErr := reflectedAgentConfig(r, o)
	if cfgErr != nil {
		return nil, cfgErr
	}

	resolved := host.MakeAgentId(r.Name(), ctor, phantom)
	if resolved.IsErr() {
		return nil, fmt.Errorf("golem: %s: %w", r.Name(), agentErrorToGo(resolved.Err()))
	}
	// The fallible form is the reflective one: a caller that built its
	// constructor from a snapshot should get an error, not a trap, when the
	// deployment has moved on.
	created := host.WasmRpcCreate(r.Name(), ctor, phantom, agentConfig)
	if created.Tag() == witTypes.ResultErr {
		return nil, rpcErrorToGo(r.Name(), "<constructor>", created.Err())
	}
	return &ReflectedAgentClient{
		agentType: r,
		agentID:   resolved.Ok(),
		rpc:       witRPC{rpc: created.Ok(), target: r.Name()},
	}, nil
}

// reflectedAgentConfig resolves creation-time configuration overrides against
// the snapshot's declarations.
func reflectedAgentConfig(r ReflectedAgentType, o clientOpts) ([]common.TypedAgentConfigValue, error) {
	if len(o.configs) == 0 {
		return nil, nil
	}
	return nil, fmt.Errorf(
		"golem: %s: configuration overrides are not available on reflected clients yet", r.Name())
}
