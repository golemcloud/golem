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
	"errors"
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
		go pumpToolStdin(writer, closed, stdin)
	}
	stdoutArg, stdout := requestOutput("stdout", streams.Stdout)
	stderrArg, stderr := requestOutput("stderr", streams.Stderr)

	future := rpc.AsyncInvokeAndAwait(slices.Clone(path), input, stdinArg, stdoutArg, stderrArg)
	return toolCall{
		stdout: stdout,
		stderr: stderr,
		wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
			res := future.Get()
			future.Drop()
			rpc.Drop()
			if res.Tag() == witTypes.ResultErr {
				e := res.Err()
				return witTypes.None[types.TypedSchemaValue](), &e
			}
			return res.Ok().Result, nil
		},
		cancel: future.Cancel,
	}, nil
}

// requestOutput creates the target and reader of an output the call requests.
func requestOutput(name string, requested bool) (witTypes.Option[*toolHost.ToolOutput], *byteReader) {
	if !requested {
		return witTypes.None[*toolHost.ToolOutput](), nil
	}
	handle, reader := toolHost.CreateOutput()
	return witTypes.Some(handle), &byteReader{src: reader, name: name, release: reader.Drop}
}

// pumpToolStdin copies the caller's reader into the call's standard input and
// selects the stream's terminal: finished at io.EOF, failed on a read error.
func pumpToolStdin(writer *toolHost.ToolStdinWriter, closed *toolHost.ToolStdinClosed, src io.Reader) {
	defer writer.Drop()
	defer closed.Drop()
	buf := make([]byte, 64*1024)
	for {
		n, err := src.Read(buf)
		if n > 0 {
			if res := writer.Write(slices.Clone(buf[:n])); res.Tag() == witTypes.ResultErr {
				return
			}
		}
		switch {
		case errors.Is(err, io.EOF):
			writer.Finish()
			return
		case err != nil:
			failure := OutputFailed(err.Error())
			var se *OutputError
			if errors.As(err, &se) {
				failure = se.Failure
			}
			writer.Fail(failure.wit)
			return
		}
	}
}
