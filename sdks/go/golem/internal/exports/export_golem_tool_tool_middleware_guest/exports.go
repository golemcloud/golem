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

// Package export_golem_tool_tool_middleware_guest holds the hand-written export
// slots for golem:tool/tool-middleware-guest@0.1.0.
//
// Same arrangement as the other export packages: the generated wasmexport glue
// calls in here, the SDK fills the slots from its init(), and a nil slot panics
// with a clear message rather than a nil dereference.
package export_golem_tool_tool_middleware_guest

import (
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	streams "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_streams"
	underlying "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_underlying"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Stdin and Stdout mirror the tool guest's stream parameters.
type Stdin = witTypes.Option[*witTypes.StreamReader[witTypes.Result[[]uint8, streams.ByteStreamFailure]]]

type Stdout = witTypes.Option[*streams.ToolStdoutWriter]

// Exports is the set of slots the SDK must fill before the component is invoked.
var Exports struct {
	DiscoverToolMiddlewares func() witTypes.Result[[]toolCommon.ToolMiddleware, types.ToolError]
	GetToolMiddleware       func(name string) witTypes.Result[toolCommon.ToolMiddleware, types.ToolError]
	InvokeToolMiddleware    func(
		middlewareName string,
		toolName string,
		toolMetadata toolCommon.Tool,
		parameters types.TypedSchemaValue,
		commandPath []string,
		input types.TypedSchemaValue,
		stdin Stdin,
		stdout Stdout,
		principal common.Principal,
		wrapped *underlying.UnderlyingTool,
	) witTypes.Result[toolCommon.InvocationResult, types.ToolError]
}

func mustBeSet(name string, set bool) {
	if !set {
		panic("golem: export " + name + " was not registered — is the SDK imported?")
	}
}

func DiscoverToolMiddlewares() witTypes.Result[[]toolCommon.ToolMiddleware, types.ToolError] {
	mustBeSet("discover-tool-middlewares", Exports.DiscoverToolMiddlewares != nil)
	return Exports.DiscoverToolMiddlewares()
}

func GetToolMiddleware(name string) witTypes.Result[toolCommon.ToolMiddleware, types.ToolError] {
	mustBeSet("get-tool-middleware", Exports.GetToolMiddleware != nil)
	return Exports.GetToolMiddleware(name)
}

func InvokeToolMiddleware(
	middlewareName string,
	toolName string,
	toolMetadata toolCommon.Tool,
	parameters types.TypedSchemaValue,
	commandPath []string,
	input types.TypedSchemaValue,
	stdin Stdin,
	stdout Stdout,
	principal common.Principal,
	wrapped *underlying.UnderlyingTool,
) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
	mustBeSet("invoke-tool-middleware", Exports.InvokeToolMiddleware != nil)
	return Exports.InvokeToolMiddleware(middlewareName, toolName, toolMetadata, parameters,
		commandPath, input, stdin, stdout, principal, wrapped)
}
