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

// A component always exports golem:tool/guest and the tool middleware guest,
// whether or not it declares any tools, because the interfaces are part of the
// world rather than something a component opts into. Until the tool package
// installs its own, the exports answer for a component with no tools: an empty
// list from discovery, and an unknown name from everything else — which is what
// a host probing for tools expects, rather than a trap.
func init() {
	toolExports.Exports.DiscoverTools = func() witTypes.Result[[]toolCommon.Tool, types.ToolError] {
		return witTypes.Ok[[]toolCommon.Tool, types.ToolError](nil)
	}
	toolExports.Exports.GetTool = func(name string) witTypes.Result[toolCommon.Tool, types.ToolError] {
		return witTypes.Err[toolCommon.Tool](types.MakeToolErrorInvalidToolName(name))
	}
	toolExports.Exports.Invoke = func(
		toolName string, _ []string, _ types.TypedSchemaValue,
		_ toolExports.Stdin, _ toolExports.Stdout, _ toolExports.Stderr, _ common.Principal,
	) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
		return witTypes.Err[toolCommon.InvocationResult](types.MakeToolErrorInvalidToolName(toolName))
	}
	mwExports.Exports.DiscoverToolMiddlewares = func() witTypes.Result[[]toolCommon.ToolMiddleware, types.ToolError] {
		return witTypes.Ok[[]toolCommon.ToolMiddleware, types.ToolError](nil)
	}
	mwExports.Exports.GetToolMiddleware = func(name string) witTypes.Result[toolCommon.ToolMiddleware, types.ToolError] {
		return witTypes.Err[toolCommon.ToolMiddleware](types.MakeToolErrorInvalidToolName(name))
	}
	mwExports.Exports.InvokeToolMiddleware = func(
		middlewareName string, _ string, _ toolCommon.Tool, _ types.TypedSchemaValue, _ []string,
		_ types.TypedSchemaValue, _ mwExports.Stdin, _ mwExports.Stdout, _ mwExports.Stderr,
		_ common.Principal, _ *underlying.UnderlyingTool,
	) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
		return witTypes.Err[toolCommon.InvocationResult](types.MakeToolErrorInvalidToolName(middlewareName))
	}
}
