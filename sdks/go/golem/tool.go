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
	byName map[string]*toolEntry
}

func newToolRegistry() *toolRegistry {
	return &toolRegistry{byName: map[string]*toolEntry{}}
}

// toolDefs is the process-wide tool registry, mirroring defs for agents.
var toolDefs = newToolRegistry()

// discover derives every registered tool's metadata, collecting problems on d
// the same way agent discovery does. It reads the registry without mutating it,
// so it is idempotent.
func (r *toolRegistry) discover(d *definitions) ([]toolCommon.Tool, bool) {
	out := make([]toolCommon.Tool, 0, len(r.order))
	ok := true
	for _, name := range r.order {
		tool, built := d.buildTool(r.byName[name])
		if !built {
			ok = false
		}
		out = append(out, tool)
	}
	return out, ok
}

func (r *toolRegistry) get(name string) (*toolEntry, bool) {
	e, ok := r.byName[name]
	return e, ok
}

func init() {
	toolExports.Exports.DiscoverTools = func() witTypes.Result[[]toolCommon.Tool, types.ToolError] {
		tools, ok := toolDefs.discover(defs)
		if !ok {
			return witTypes.Err[[]toolCommon.Tool](toolDefinitionError(defs))
		}
		return witTypes.Ok[[]toolCommon.Tool, types.ToolError](tools)
	}

	toolExports.Exports.GetTool = func(name string) witTypes.Result[toolCommon.Tool, types.ToolError] {
		e, ok := toolDefs.get(name)
		if !ok {
			return witTypes.Err[toolCommon.Tool](types.MakeToolErrorInvalidToolName(name))
		}
		tool, built := defs.buildTool(e)
		if !built {
			return witTypes.Err[toolCommon.Tool](toolDefinitionError(defs))
		}
		return witTypes.Ok[toolCommon.Tool, types.ToolError](tool)
	}

	toolExports.Exports.Invoke = func(
		toolName string,
		commandPath []string,
		input types.TypedSchemaValue,
		stdin toolExports.Stdin,
		stdout toolExports.Stdout,
		_ common.Principal,
	) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
		e, ok := toolDefs.get(toolName)
		if !ok {
			return witTypes.Err[toolCommon.InvocationResult](types.MakeToolErrorInvalidToolName(toolName))
		}
		return defs.invokeCommand(e, commandPath, input, newToolStdin(stdin), newToolStdout(stdout))
	}
}

// ToolContext is the per-invocation context handed to a command handler.
type ToolContext struct {
	tool   string
	path   []string
	stdin  *ToolStdin
	stdout *ToolStdout
}

// Tool returns the name of the tool being invoked.
func (c *ToolContext) Tool() string { return c.tool }

// CommandPath returns the path of the command being invoked, from the tool's
// root; empty means the root command's own body.
func (c *ToolContext) CommandPath() []string { return append([]string(nil), c.path...) }

// Stdin returns the command's standard input. It is never nil: a command that
// did not declare a stdin stream, or that the host invoked without one, gets a
// reader whose every Read explains that rather than a nil dereference.
func (c *ToolContext) Stdin() *ToolStdin { return c.stdin }

// Stdout returns the command's standard output. It is never nil, on the same
// terms as [ToolContext.Stdin]. The stream is finished when the handler returns
// and failed when it panics.
func (c *ToolContext) Stdout() *ToolStdout { return c.stdout }

// toolDefinitionError reports a broken tool declaration. The WIT has no variant
// for "this component's own metadata is wrong"; invalid-result is the closest,
// and is what the TypeScript SDK uses for the same case. custom-error would not
// do: its name field is the tool's own declared error case, and a definition
// failure is not one of those.
func toolDefinitionError(d *definitions) types.ToolError {
	return types.MakeToolErrorInvalidResult("tool definition errors:\n" + allDefErrors(d.errs))
}
