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

package tool

import (
	"io"
	"slices"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_host"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// startToolCallHost invokes through the host's tool RPC resource. It awaits
// through the asynchronous import, for the same reason [MethodDef.Call] does:
// it is the form a suspended caller is resumed into, and it lets the stdin pump
// run while the call is in flight.
func startToolCallHost(
	tool string, path []string, input types.TypedSchemaValue, stdin io.Reader, streams Streams,
) (toolCall, error) {
	created := toolHost.ToolRpcCreate(tool)
	if created.Tag() == witTypes.ResultErr {
		return toolCall{}, toolCallErrorFromWit(tool, path, created.Err())
	}
	rpc := created.Ok()

	stdinArg := witTypes.None[*toolHost.ToolStdin]()
	if stdin != nil {
		writer, handle, closed := toolHost.CreateStdin()
		stdinArg = witTypes.Some(handle)
		go pumpStdin(stdin, hostStdin{writer}, func() { closed.Wait() }, closed.Drop)
	}
	stdoutArg, stdout := requestOutput("stdout", streams.Stdout)
	stderrArg, stderr := requestOutput("stderr", streams.Stderr)

	future := rpc.AsyncInvokeAndAwait(slices.Clone(path), input, stdinArg, stdoutArg, stderrArg)
	return guardCancel(toolCall{
		stdout: stdout,
		stderr: stderr,
		wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
			res := future.Get()
			if res.Tag() == witTypes.ResultErr {
				e := res.Err()
				return witTypes.None[types.TypedSchemaValue](), &e
			}
			return res.Ok().Result, nil
		},
		cancel: future.Cancel,
		drop:   func() { defer rpc.Drop(); future.Drop() },
	}), nil
}

// requestOutput creates the target and reader of an output the call requests.
func requestOutput(name string, requested bool) (witTypes.Option[*toolHost.ToolOutput], *byteReader) {
	if !requested {
		return witTypes.None[*toolHost.ToolOutput](), nil
	}
	handle, reader := toolHost.CreateOutput()
	return witTypes.Some(handle), &byteReader{src: reader, name: name, release: reader.Drop}
}

// hostStdin is the host's standard input writer of a call.
type hostStdin struct{ w *toolHost.ToolStdinWriter }

func (h hostStdin) write(p []byte) bool {
	return h.w.Write(p).Tag() != witTypes.ResultErr
}

func (h hostStdin) finish()              { h.w.Finish() }
func (h hostStdin) fail(f OutputFailure) { h.w.Fail(f.wit) }
func (h hostStdin) release()             { h.w.Drop() }
