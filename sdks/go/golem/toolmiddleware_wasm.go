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
	"errors"
	"io"
	"slices"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	streams "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_streams"
	underlying "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_underlying"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

type byteStream = witTypes.StreamReader[witTypes.Result[[]uint8, streams.ByteStreamFailure]]

// newUnderlyingLayer binds the layer beneath a middleware to the host's
// resource.
func newUnderlyingLayer(tool *underlying.UnderlyingTool) underlyingLayer {
	if tool == nil {
		return absentUnderlyingLayer
	}
	return underlyingLayer{start: func(path []string, input types.TypedSchemaValue, stdin io.Reader) (toolCall, error) {
		res, out, errOut := tool.Invoke(slices.Clone(path), input, underlyingStdin(stdin))
		return toolCall{
			stdout: optionalReader(out),
			stderr: optionalReader(errOut),
			wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
				r := res.Get()
				res.Drop()
				if r.Tag() == witTypes.ResultErr {
					e := underlyingErrorToRPC(r.Err())
					return witTypes.None[types.TypedSchemaValue](), &e
				}
				return r.Ok(), nil
			},
			cancel: res.Cancel,
		}, nil
	}}
}

func optionalReader(stream witTypes.Option[*byteStream]) *byteReader {
	if stream.IsNone() {
		return nil
	}
	s := stream.Some()
	return &byteReader{src: s, release: s.Drop}
}

// underlyingStdin hands an unread standard input on whole, and otherwise
// streams whatever the reader yields.
func underlyingStdin(stdin io.Reader) witTypes.Option[*byteStream] {
	if stdin == nil {
		return witTypes.None[*byteStream]()
	}
	if br, ok := stdin.(*byteReader); ok && !br.consumed {
		if raw, ok := br.src.(*byteStream); ok {
			br.consumed = true
			return witTypes.Some(raw)
		}
	}
	writer, reader := underlying.MakeStreamResultListU8GolemToolStreamsByteStreamFailure()
	go pumpStream(writer, stdin)
	return witTypes.Some(reader)
}

// pumpStream copies a reader into a byte stream, ending it at io.EOF and with
// a failure item on a read error.
func pumpStream(w *witTypes.StreamWriter[witTypes.Result[[]uint8, streams.ByteStreamFailure]], src io.Reader) {
	defer w.Drop()
	buf := make([]byte, 64*1024)
	for {
		n, err := src.Read(buf)
		if n > 0 {
			item := witTypes.Ok[[]uint8, streams.ByteStreamFailure](slices.Clone(buf[:n]))
			if w.WriteAll([]witTypes.Result[[]uint8, streams.ByteStreamFailure]{item}) == 0 {
				return
			}
		}
		switch {
		case errors.Is(err, io.EOF):
			return
		case err != nil:
			failure := StreamFailed(err.Error())
			var se *StreamError
			if errors.As(err, &se) {
				failure = se.Failure
			}
			w.WriteAll([]witTypes.Result[[]uint8, streams.ByteStreamFailure]{
				witTypes.Err[[]uint8](failure.wit),
			})
			return
		}
	}
}

// underlyingErrorToRPC classifies a failure of the layer beneath the same way
// as a failed tool call.
func underlyingErrorToRPC(e underlying.UnderlyingError) types.ToolRpcError {
	switch e.Tag() {
	case underlying.UnderlyingErrorToolError:
		return types.MakeToolRpcErrorRemoteToolError(e.ToolError())
	case underlying.UnderlyingErrorProtocolError:
		return types.MakeToolRpcErrorProtocolError(e.ProtocolError())
	case underlying.UnderlyingErrorDenied:
		return types.MakeToolRpcErrorDenied(e.Denied())
	case underlying.UnderlyingErrorInternalError:
		return types.MakeToolRpcErrorRemoteInternalError(e.InternalError())
	case underlying.UnderlyingErrorCancelled:
		return types.MakeToolRpcErrorCancelled()
	case underlying.UnderlyingErrorResourceExhausted:
		return types.MakeToolRpcErrorResourceExhausted(e.ResourceExhausted())
	}
	return types.MakeToolRpcErrorProtocolError("unknown failure of the layer beneath")
}
