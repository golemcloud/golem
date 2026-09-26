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
	mwExports "github.com/golemcloud/golem/sdks/go/golem/internal/exports/export_golem_tool_tool_middleware_guest"
	streams "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_streams"
	underlying "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_underlying"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// witNextLayer invokes the runtime-owned resource standing for the layer
// beneath a middleware.
type witNextLayer struct{ tool *underlying.UnderlyingTool }

func (n witNextLayer) invoke(commandPath []string, input TypedValue, stdin nextStdin) MiddlewareOutcome {
	forwarded := witTypes.None[*witTypes.StreamReader[witTypes.Result[[]uint8, streams.ByteStreamFailure]]]()
	if s, ok := stdin.wit.(mwExports.Stdin); ok {
		forwarded = s
	}
	call, out := n.tool.Invoke(commandPath, input.wit, forwarded)
	// The result is awaited after the stdout reader is in hand, which is what
	// lets a middleware relay output while the inner layer is still running.
	res := call.Get()
	outcome := MiddlewareOutcome{}
	if out.IsSome() {
		outcome.stdout = &ToolStdin{src: out.Some()}
	}
	if res.Tag() == witTypes.ResultErr {
		outcome.err = underlyingErrorToGo(res.Err())
		return outcome
	}
	if v := res.Ok(); v.IsSome() {
		outcome.result, outcome.hasResult = TypedValue{wit: v.Some()}, true
	}
	return outcome
}

func newNextLayer(tool *underlying.UnderlyingTool) nextLayer {
	if tool == nil {
		return absentNextLayer{}
	}
	return witNextLayer{tool: tool}
}

func newNextStdin(stdin mwExports.Stdin) nextStdin { return nextStdin{wit: stdin} }
