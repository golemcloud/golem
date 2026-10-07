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

package tool

import (
	"errors"
	"fmt"
	"io"

	streams "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_streams"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Tool byte streams.
//
// A command reads standard input from the io.Reader field it binds with
// [ToolCommandSpec.Stdin], and an output command writes through
// [ToolOutputContext.Stdout] and [ToolOutputContext.Stderr], ordinary
// io.Writers — p3 makes the underlying host calls blocking, so there is no
// callback or future to thread through.
//
// The wire protocol has explicit terminals the author does not have to
// remember: a handler that succeeds finishes its output streams, and one that
// fails or panics fails them. Call [ToolOutput.Fail] to end one with a
// specific cause instead.

// absentStdin explains a standard input the host did not supply.
const absentStdin = "golem: this command was invoked without a stdin stream"

// ErrOutputDrained is what reading a tool call's output fails with when
// [Invocation.Wait] already drained it, because it was not taken before.
var ErrOutputDrained = errors.New("golem: the output was drained by Wait before it was taken")

// OutputFailure is a recoverable failure carried by a byte stream. Clean end of
// input is not a failure: it arrives as io.EOF.
type OutputFailure struct{ wit streams.ByteStreamFailure }

// OutputCancelled reports that the transfer was cancelled before completion.
func OutputCancelled() OutputFailure {
	return OutputFailure{streams.MakeByteStreamFailureCancelled()}
}

// OutputAbandoned reports that the producer went away without finishing.
func OutputAbandoned() OutputFailure {
	return OutputFailure{streams.MakeByteStreamFailureAbandoned()}
}

// OutputResourceExhausted reports that a quota or buffer was exceeded.
func OutputResourceExhausted() OutputFailure {
	return OutputFailure{streams.MakeByteStreamFailureResourceExhausted()}
}

// OutputFailed reports a source failure with a human-readable reason.
func OutputFailed(reason string) OutputFailure {
	return OutputFailure{streams.MakeByteStreamFailureFailed(reason)}
}

func (f OutputFailure) String() string {
	switch f.wit.Tag() {
	case streams.ByteStreamFailureCancelled:
		return "cancelled"
	case streams.ByteStreamFailureAbandoned:
		return "abandoned"
	case streams.ByteStreamFailureResourceExhausted:
		return "resource exhausted"
	case streams.ByteStreamFailureFailed:
		return "failed: " + f.wit.Failed()
	}
	return "unknown stream failure"
}

// OutputError reports a stream failure that arrived as a value rather than as
// the end of the stream. Distinguish it from io.EOF with errors.As.
type OutputError struct{ Failure OutputFailure }

func (e *OutputError) Error() string { return "golem: stream " + e.Failure.String() }

// byteStreamSource is the reading half of a tool stdin stream. The WIT reader
// satisfies it; the narrow interface keeps the end-of-stream and failure
// handling testable without a host.
type byteStreamSource interface {
	Read(dst []witTypes.Result[[]uint8, streams.ByteStreamFailure]) uint32
	WriterDropped() bool
}

// byteStreamSink is the writing half of a tool stdout stream, with the three
// operations the WIT resource offers.
type byteStreamSink interface {
	Write(bytes []uint8) witTypes.Result[witTypes.Unit, streams.StreamWriteError]
	Finish() witTypes.Result[witTypes.Unit, streams.StreamWriteError]
	Fail(reason streams.ByteStreamFailure) witTypes.Result[witTypes.Unit, streams.StreamWriteError]
}

// byteReader is a byte stream read as an ordinary io.Reader: a command's
// standard input, or the standard output of a tool it called.
type byteReader struct {
	src byteStreamSource
	// absent explains why there is no stream, so a Read says so rather than
	// dereferencing nil.
	absent string
	// pending holds the remainder of a chunk that did not fit the caller's
	// buffer, since the wire delivers whole chunks.
	pending []byte
	done    bool
	// consumed records that reading began, after which the stream can no
	// longer be handed on whole.
	consumed bool
	// release drops the underlying stream, when it is a host resource.
	release func()
}

// close releases the stream unread.
func (r *byteReader) close() {
	if r.release != nil {
		r.release()
	}
	r.done = true
}

// present reports whether the host supplied the stream.
func (r *byteReader) present() bool { return r != nil && r.absent == "" }

// Read fills p from the stream. It returns io.EOF at clean end of input, and a
// [OutputError] when the producer reports a failure.
func (r *byteReader) Read(p []byte) (int, error) {
	r.consumed = true
	if r.absent != "" {
		return 0, errors.New(r.absent)
	}
	if len(p) == 0 {
		return 0, nil
	}
	if len(r.pending) > 0 {
		n := copy(p, r.pending)
		r.pending = r.pending[n:]
		return n, nil
	}
	if r.done {
		return 0, io.EOF
	}
	// One item at a time: an item is either a whole chunk or a failure, and a
	// failure must be reported before anything after it is consumed.
	items := make([]witTypes.Result[[]uint8, streams.ByteStreamFailure], 1)
	for {
		count := r.src.Read(items)
		if count == 0 {
			if r.src.WriterDropped() {
				r.done = true
				return 0, io.EOF
			}
			continue
		}
		item := items[0]
		if item.Tag() == witTypes.ResultErr {
			r.done = true
			return 0, &OutputError{Failure: OutputFailure{item.Err()}}
		}
		chunk := item.Ok()
		if len(chunk) == 0 {
			// Every successful item carries a non-empty chunk, but an empty one
			// is harmless: skip it rather than reporting a spurious EOF.
			continue
		}
		n := copy(p, chunk)
		r.pending = append(r.pending[:0], chunk[n:]...)
		return n, nil
	}
}

// Output is a command's standard output or standard error, written as an
// ordinary io.Writer.
//
// The stream is finished when the handler returns and failed when it fails or
// panics, so nothing has to be closed by hand. [Output.Fail] ends it with a
// specific cause instead; the first terminal wins and later ones are ignored.
//
// An optional output the caller did not ask for discards what is written to it;
// [Output.Attached] tells the two apart. Writing to an output the command
// does not declare is an error.
type Output struct {
	name string
	// sink is the host's writer, nil when the caller did not attach the stream.
	sink byteStreamSink
	// undeclared explains why the command has no such output, so a Write says
	// so instead of discarding silently.
	undeclared string
	// terminal records that finish or fail already ran, since the wire accepts
	// exactly one and the SDK also selects one automatically.
	terminal bool
}

// newToolOutput is a declared output, attached when sink is not nil.
func newToolOutput(name string, sink byteStreamSink) *Output {
	return &Output{name: name, sink: sink}
}

// Attached reports whether the caller receives what is written; an optional
// output the caller did not ask for discards it.
func (w *Output) Attached() bool { return w.sink != nil }

// Write sends bytes to the stream.
func (w *Output) Write(p []byte) (int, error) {
	if w.undeclared != "" {
		return 0, errors.New(w.undeclared)
	}
	if w.terminal {
		return 0, fmt.Errorf("golem: %s is already finished", w.name)
	}
	if len(p) == 0 || w.sink == nil {
		return len(p), nil
	}
	if res := w.sink.Write(p); res.Tag() == witTypes.ResultErr {
		return 0, writeError(w.name, res.Err())
	}
	return len(p), nil
}

// Fail ends the stream with the given cause instead of finishing it cleanly.
func (w *Output) Fail(cause OutputFailure) error {
	if w.undeclared != "" {
		return errors.New(w.undeclared)
	}
	if w.terminal {
		return nil
	}
	w.terminal = true
	if w.sink == nil {
		return nil
	}
	if res := w.sink.Fail(cause.wit); res.Tag() == witTypes.ResultErr {
		return writeError(w.name, res.Err())
	}
	return nil
}

// finish selects the clean terminal, unless one was already selected. A
// stream the host supplied for an undeclared output is finished empty.
func (w *Output) finish() error {
	if w.terminal {
		return nil
	}
	w.terminal = true
	if w.sink == nil {
		return nil
	}
	if res := w.sink.Finish(); res.Tag() == witTypes.ResultErr {
		return writeError(w.name, res.Err())
	}
	return nil
}

// fail is Fail for the SDK's own use, which also ends a stream the host
// supplied for an undeclared output.
func (w *Output) fail(cause OutputFailure) {
	if w.terminal {
		return
	}
	w.terminal = true
	if w.sink != nil {
		w.sink.Fail(cause.wit)
	}
}

func writeError(name string, e streams.StreamWriteError) error {
	switch e.Tag() {
	case streams.StreamWriteErrorConcurrentOperation:
		return fmt.Errorf("golem: concurrent operation on the %s stream", name)
	case streams.StreamWriteErrorClosed:
		cause := e.Closed()
		switch cause.Tag() {
		case streams.ByteStreamCloseCauseFinished:
			return fmt.Errorf("golem: %s stream is already finished", name)
		case streams.ByteStreamCloseCauseConsumerCancelled:
			return fmt.Errorf("golem: the consumer cancelled the %s stream", name)
		case streams.ByteStreamCloseCauseFailed:
			return &OutputError{Failure: OutputFailure{cause.Failed()}}
		}
	}
	return fmt.Errorf("golem: %s stream write failed", name)
}
