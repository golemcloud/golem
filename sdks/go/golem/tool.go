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
	toolExports "github.com/golemcloud/golem/sdks/go/golem/internal/exports/export_golem_tool_guest"
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Tools.
//
// A component always exports golem:tool/guest, whether or not it declares any
// tools, because the interface is part of the world rather than something a
// component opts into. A component with no tools therefore answers
// discover-tools with an empty list, and reports an unknown name from get-tool
// and invoke — which is what a host probing for tools expects, rather than a
// trap.

// tools holds the registered tool definitions, keyed by the tool's name (its
// root command name). It is filled at registration time, before the component
// is invoked, so it needs no locking for the same reason the agent definitions
// do not.
type toolRegistry struct {
	order  []string
	byName map[string]*toolDef
}

// toolDef is one registered tool: the metadata the host discovers, plus the
// handlers invoke dispatches to.
type toolDef struct {
	name string
	tool toolCommon.Tool
}

func newToolRegistry() *toolRegistry {
	return &toolRegistry{byName: map[string]*toolDef{}}
}

// toolDefs is the process-wide tool registry, mirroring defs for agents.
var toolDefs = newToolRegistry()

func (r *toolRegistry) discover() []toolCommon.Tool {
	out := make([]toolCommon.Tool, 0, len(r.order))
	for _, name := range r.order {
		out = append(out, r.byName[name].tool)
	}
	return out
}

func (r *toolRegistry) get(name string) (*toolDef, bool) {
	d, ok := r.byName[name]
	return d, ok
}

func init() {
	toolExports.Exports.DiscoverTools = func() witTypes.Result[[]toolCommon.Tool, types.ToolError] {
		return witTypes.Ok[[]toolCommon.Tool, types.ToolError](toolDefs.discover())
	}

	toolExports.Exports.GetTool = func(name string) witTypes.Result[toolCommon.Tool, types.ToolError] {
		d, ok := toolDefs.get(name)
		if !ok {
			return witTypes.Err[toolCommon.Tool](types.MakeToolErrorInvalidToolName(name))
		}
		return witTypes.Ok[toolCommon.Tool, types.ToolError](d.tool)
	}

	toolExports.Exports.Invoke = func(
		toolName string,
		commandPath []string,
		_ types.TypedSchemaValue,
		_ toolExports.Stdin,
		_ toolExports.Stdout,
		_ common.Principal,
	) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
		if _, ok := toolDefs.get(toolName); !ok {
			return witTypes.Err[toolCommon.InvocationResult](types.MakeToolErrorInvalidToolName(toolName))
		}
		return witTypes.Err[toolCommon.InvocationResult](types.MakeToolErrorInvalidCommandPath(commandPath))
	}
}
