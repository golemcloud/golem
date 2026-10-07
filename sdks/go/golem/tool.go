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
	mwExports "github.com/golemcloud/golem/sdks/go/golem/internal/exports/export_golem_tool_tool_middleware_guest"
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	underlying "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_underlying"
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
	// remote holds the tools declared only to be called, so a name is not
	// declared twice across both kinds.
	remote map[string]*toolEntry
	// Middleware is registered alongside tools: a component may export both.
	middlewareOrder   []string
	middlewaresByName map[string]*middlewareEntry
}

func newToolRegistry() *toolRegistry {
	return &toolRegistry{
		byName:            map[string]*toolEntry{},
		remote:            map[string]*toolEntry{},
		middlewaresByName: map[string]*middlewareEntry{},
	}
}

// discoverMiddlewares derives every registered middleware's metadata.
func (r *toolRegistry) discoverMiddlewares(d *definitions) ([]toolCommon.ToolMiddleware, bool) {
	out := make([]toolCommon.ToolMiddleware, 0, len(r.middlewareOrder))
	ok := true
	for _, name := range r.middlewareOrder {
		m, built := d.buildToolMiddleware(r.middlewaresByName[name])
		if !built {
			ok = false
		}
		out = append(out, m)
	}
	return out, ok
}

func (r *toolRegistry) getMiddleware(name string) (*middlewareEntry, bool) {
	e, ok := r.middlewaresByName[name]
	return e, ok
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
		stderr toolExports.Stderr,
		principal common.Principal,
	) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
		e, ok := toolDefs.get(toolName)
		if !ok {
			return witTypes.Err[toolCommon.InvocationResult](types.MakeToolErrorInvalidToolName(toolName))
		}
		return defs.invokeCommand(e, commandPath, input, newToolStdin(stdin),
			hostOutputs{stdout: hostOutput(stdout), stderr: hostOutput(toolExports.Stdout(stderr))},
			principalFromWit(principal))
	}

	mwExports.Exports.DiscoverToolMiddlewares = func() witTypes.Result[[]toolCommon.ToolMiddleware, types.ToolError] {
		found, ok := toolDefs.discoverMiddlewares(defs)
		if !ok {
			return witTypes.Err[[]toolCommon.ToolMiddleware](toolDefinitionError(defs))
		}
		return witTypes.Ok[[]toolCommon.ToolMiddleware, types.ToolError](found)
	}

	mwExports.Exports.GetToolMiddleware = func(name string) witTypes.Result[toolCommon.ToolMiddleware, types.ToolError] {
		e, ok := toolDefs.getMiddleware(name)
		if !ok {
			return witTypes.Err[toolCommon.ToolMiddleware](types.MakeToolErrorInvalidToolName(name))
		}
		m, built := defs.buildToolMiddleware(e)
		if !built {
			return witTypes.Err[toolCommon.ToolMiddleware](toolDefinitionError(defs))
		}
		return witTypes.Ok[toolCommon.ToolMiddleware, types.ToolError](m)
	}

	mwExports.Exports.InvokeToolMiddleware = func(
		middlewareName string,
		toolName string,
		toolMetadata toolCommon.Tool,
		parameters types.TypedSchemaValue,
		commandPath []string,
		input types.TypedSchemaValue,
		stdin mwExports.Stdin,
		stdout mwExports.Stdout,
		stderr mwExports.Stderr,
		principal common.Principal,
		wrapped *underlying.UnderlyingTool,
	) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
		return defs.invokeMiddleware(middlewareName, &middlewareInvocation{
			toolName:    toolName,
			tool:        toolMetadata,
			parameters:  TypedValue{wit: parameters},
			commandPath: commandPath,
			input:       input,
			stdin:       newToolStdin(toolExports.Stdin(stdin)),
			stdout:      newToolOutput("stdout", hostOutput(toolExports.Stdout(stdout))),
			stderr:      newToolOutput("stderr", hostOutput(toolExports.Stdout(stderr))),
			principal:   principalFromWit(principal),
			under:       newUnderlyingLayer(wrapped),
		})
	}
}

// ToolContext is the per-invocation context handed to a command handler. The
// arguments, standard input and principal arrive in the argument struct; the
// context says which command is running.
type ToolContext struct {
	tool string
	path []string
}

// Tool returns the name of the tool being invoked.
func (c *ToolContext) Tool() string { return c.tool }

// CommandPath returns the path of the command being invoked, from the tool's
// root; empty means the root command's own body.
func (c *ToolContext) CommandPath() []string { return append([]string(nil), c.path...) }

// ToolOutputContext is the context handed to a command declared with
// OutputCommand, which also carries its outputs.
type ToolOutputContext struct {
	ToolContext
	stdout, stderr *ToolOutput
}

// Stdout returns the command's standard output. The stream is finished when
// the handler succeeds and failed when it returns an error or panics.
func (c *ToolOutputContext) Stdout() *ToolOutput { return c.stdout }

// Stderr returns the command's standard error, finished and failed like
// Stdout. Bytes on it do not mean the command failed.
func (c *ToolOutputContext) Stderr() *ToolOutput { return c.stderr }

// toolDefinitionError reports a broken tool declaration. The WIT has no variant
// for "this component's own metadata is wrong"; invalid-result is the closest,
// and is what the TypeScript SDK uses for the same case. custom-error would not
// do: its name field is the tool's own declared error case, and a definition
// failure is not one of those.
func toolDefinitionError(d *definitions) types.ToolError {
	return types.MakeToolErrorInvalidResult("tool definition errors:\n" + allDefErrors(d.Errs))
}
