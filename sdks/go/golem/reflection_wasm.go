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
	toolHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_host"
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

// DiscoverTools returns a snapshot of every tool the calling agent may reach in
// its environment.
func DiscoverTools() []ReflectedTool {
	registered := toolHost.GetAllTools()
	out := make([]ReflectedTool, 0, len(registered))
	for _, r := range registered {
		out = append(out, ReflectedTool{lookupName: r.LookupName, wit: r.Definition})
	}
	return out
}

// DiscoverTool looks one tool up by its lookup name.
func DiscoverTool(name string) (ReflectedTool, bool) {
	found := toolHost.GetTool(name)
	if found.IsNone() {
		return ReflectedTool{}, false
	}
	return ReflectedTool{lookupName: found.Some().LookupName, wit: found.Some().Definition}, true
}

// witToolRPC invokes through the host's tool RPC resource.
type witToolRPC struct {
	rpc  *toolHost.ToolRpc
	name string
}

func (w witToolRPC) invokeAndAwait(commandPath []string, input types.TypedSchemaValue) (types.TypedSchemaValue, bool, error) {
	res := w.rpc.InvokeAndAwait(commandPath, input,
		witTypes.None[*toolHost.ToolStdin](), witTypes.None[*toolHost.ToolStdout]())
	if res.Tag() == witTypes.ResultErr {
		return types.TypedSchemaValue{}, false, fmt.Errorf("golem: tool %s %s: %s",
			w.name, commandLabel(commandPath), toolRPCErrorMessage(res.Err()))
	}
	out := res.Ok().Result
	if out.IsNone() {
		return types.TypedSchemaValue{}, false, nil
	}
	return out.Some(), true, nil
}

// Bind connects to the discovered tool.
func (r ReflectedTool) Bind() (*ReflectedToolClient, error) {
	// The fallible form is the reflective one: a snapshot that has gone stale
	// should give an error rather than a trap.
	created := toolHost.ToolRpcCreate(r.lookupName)
	if created.Tag() == witTypes.ResultErr {
		return nil, fmt.Errorf("golem: tool %s: %s", r.lookupName, toolRPCErrorMessage(created.Err()))
	}
	return &ReflectedToolClient{tool: r, rpc: witToolRPC{rpc: created.Ok(), name: r.lookupName}}, nil
}

// ParseRawAgentID takes an agent identity apart without decoding its
// constructor into a Go type. Parsing is strict: a malformed identity is
// reported rather than guessed at.
func ParseRawAgentID(agentID string) (RawAgentID, error) {
	res := host.ParseAgentId(agentID)
	if res.IsErr() {
		return RawAgentID{}, fmt.Errorf("golem: parsing agent id %q: %w", agentID, agentErrorToGo(res.Err()))
	}
	t := res.Ok()
	phantom := None[UUID]()
	if t.F2.IsSome() {
		phantom = Some(uuidFromWit(t.F2.Some()))
	}
	return RawAgentID{AgentType: t.F0, Constructor: TypedValue{wit: t.F1}, Phantom: phantom}, nil
}

// BindAgentID binds an existing agent identity. Binding never creates the
// agent, and makes no claim about its type beyond what the identity says.
func BindAgentID(agentID string) (*DynamicAgentClient, error) {
	parsed, err := ParseRawAgentID(agentID)
	if err != nil {
		return nil, err
	}
	phantom := witTypes.None[types.Uuid]()
	if id, present := parsed.Phantom.Get(); present {
		phantom = witTypes.Some(uuidToWit(id))
	}
	created := host.WasmRpcCreate(parsed.AgentType, parsed.Constructor.wit.Value, phantom, nil)
	if created.Tag() == witTypes.ResultErr {
		return nil, rpcErrorToGo(parsed.AgentType, "<bind>", created.Err())
	}
	return &DynamicAgentClient{
		agentID: agentID,
		parsed:  parsed,
		rpc:     witRPC{rpc: created.Ok(), target: parsed.AgentType},
	}, nil
}

// BindTool binds a tool by name without retaining its metadata.
func BindTool(toolName string) (*DynamicToolClient, error) {
	created := toolHost.ToolRpcCreate(toolName)
	if created.Tag() == witTypes.ResultErr {
		return nil, fmt.Errorf("golem: tool %s: %s", toolName, toolRPCErrorMessage(created.Err()))
	}
	return &DynamicToolClient{
		toolName: toolName,
		rpc:      witToolRPC{rpc: created.Ok(), name: toolName},
	}, nil
}
