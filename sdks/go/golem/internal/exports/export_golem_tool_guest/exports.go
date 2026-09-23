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

// Package export_golem_tool_guest holds the hand-written export slots for
// golem:tool/guest@0.1.0.
//
// Same arrangement as export_golem_agent_guest: the generated wasmexport glue
// in internal/wit/wit_exports calls into this package, the SDK fills the slots
// from its init(), and a nil slot panics with a clear message. A component that
// declares no tools still exports the interface, so discover-tools answers with
// an empty list rather than trapping.
package export_golem_tool_guest

import (
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	streams "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_streams"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Stdin is the reader supplied when the selected command body declared a stdin
// stream; absent otherwise.
type Stdin = witTypes.Option[*witTypes.StreamReader[witTypes.Result[[]uint8, streams.ByteStreamFailure]]]

// Stdout is the writer supplied when the selected command body declared a
// stdout stream; absent otherwise.
type Stdout = witTypes.Option[*streams.ToolStdoutWriter]

// Exports is the set of slots the SDK must fill before the component is invoked.
var Exports struct {
	DiscoverTools func() witTypes.Result[[]toolCommon.Tool, types.ToolError]
	GetTool       func(name string) witTypes.Result[toolCommon.Tool, types.ToolError]
	Invoke        func(toolName string, commandPath []string, input types.TypedSchemaValue, stdin Stdin, stdout Stdout, principal common.Principal) witTypes.Result[toolCommon.InvocationResult, types.ToolError]
}

func mustBeSet(name string, set bool) {
	if !set {
		panic("golem: export " + name + " was not registered — is the SDK imported?")
	}
}

func DiscoverTools() witTypes.Result[[]toolCommon.Tool, types.ToolError] {
	mustBeSet("discover-tools", Exports.DiscoverTools != nil)
	return Exports.DiscoverTools()
}

func GetTool(name string) witTypes.Result[toolCommon.Tool, types.ToolError] {
	mustBeSet("get-tool", Exports.GetTool != nil)
	return Exports.GetTool(name)
}

func Invoke(toolName string, commandPath []string, input types.TypedSchemaValue, stdin Stdin, stdout Stdout, principal common.Principal) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
	mustBeSet("invoke", Exports.Invoke != nil)
	return Exports.Invoke(toolName, commandPath, input, stdin, stdout, principal)
}
