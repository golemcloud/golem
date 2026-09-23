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
	"errors"
	"fmt"
	"io"

	streams "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_streams"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Tool byte streams.
//
// A command that declared a stdin or stdout stream reads and writes it through
// [ToolContext.Stdin] and [ToolContext.Stdout], which are an ordinary io.Reader
// and io.Writer — p3 makes the underlying host calls blocking, so there is no
// callback or future to thread through.
//
// The wire protocol has explicit terminals the author does not have to
// remember: returning from the handler finishes the output stream, and panicking
// fails it. Call [ToolStdout.Fail] to end it with a specific cause instead.

// Messages explaining a stream that the host did not supply, shared by the
// per-target constructors.
const (
	absentStdin  = "golem: this command was invoked without a stdin stream; declare one with golem.Stdin"
	absentStdout = "golem: this command was invoked without a stdout stream; declare one with golem.Stdout"
)

// StreamFailure is a recoverable failure carried by a byte stream. Clean end of
// input is not a failure: it arrives as io.EOF.
type StreamFailure struct{ wit streams.ByteStreamFailure }

// StreamCancelled reports that the transfer was cancelled before completion.
func StreamCancelled() StreamFailure {
	return StreamFailure{streams.MakeByteStreamFailureCancelled()}
}

// StreamAbandoned reports that the producer went away without finishing.
func StreamAbandoned() StreamFailure {
	return StreamFailure{streams.MakeByteStreamFailureAbandoned()}
}

// StreamResourceExhausted reports that a quota or buffer was exceeded.
func StreamResourceExhausted() StreamFailure {
	return StreamFailure{streams.MakeByteStreamFailureResourceExhausted()}
}

// StreamFailed reports a source failure with a human-readable reason.
func StreamFailed(reason string) StreamFailure {
	return StreamFailure{streams.MakeByteStreamFailureFailed(reason)}
}

func (f StreamFailure) String() string {
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

// Error reports a stream failure that arrived as a value rather than as the end
// of the stream. Distinguish it from io.EOF with errors.As.
type StreamError struct{ Failure StreamFailure }

func (e *StreamError) Error() string { return "golem: stream " + e.Failure.String() }

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

// ToolStdin is a command's standard input, read as an ordinary io.Reader.
type ToolStdin struct {
	src byteStreamSource
	// absent explains why there is no stream, so a Read says so rather than
	// dereferencing nil.
	absent string
	// pending holds the remainder of a chunk that did not fit the caller's
	// buffer, since the wire delivers whole chunks.
	pending []byte
	done    bool
}

// Read fills p from the stream. It returns io.EOF at clean end of input, and a
// [StreamError] when the producer reports a failure.
func (r *ToolStdin) Read(p []byte) (int, error) {
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
			return 0, &StreamError{Failure: StreamFailure{item.Err()}}
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

// ToolStdout is a command's standard output, written as an ordinary io.Writer.
//
// The stream is finished when the handler returns and failed when it panics, so
// nothing has to be closed by hand. [ToolStdout.Fail] ends it with a specific
// cause instead; the first terminal wins and later ones are ignored.
type ToolStdout struct {
	sink byteStreamSink
	// absent explains why there is no stream, so a Write says so rather than
	// dereferencing nil.
	absent string
	// terminal records that finish or fail already ran, since the wire accepts
	// exactly one and the SDK also selects one automatically.
	terminal bool
}

// Write sends bytes to the stream.
func (w *ToolStdout) Write(p []byte) (int, error) {
	if w.absent != "" {
		return 0, errors.New(w.absent)
	}
	if w.terminal {
		return 0, errors.New("golem: stdout is already finished")
	}
	if len(p) == 0 {
		return 0, nil
	}
	if res := w.sink.Write(p); res.Tag() == witTypes.ResultErr {
		return 0, writeError(res.Err())
	}
	return len(p), nil
}

// Fail ends the stream with the given cause instead of finishing it cleanly.
func (w *ToolStdout) Fail(cause StreamFailure) error {
	if w.absent != "" {
		return errors.New(w.absent)
	}
	if w.terminal {
		return nil
	}
	w.terminal = true
	if res := w.sink.Fail(cause.wit); res.Tag() == witTypes.ResultErr {
		return writeError(res.Err())
	}
	return nil
}

// finish selects the clean terminal, unless one was already selected.
func (w *ToolStdout) finish() error {
	if w.absent != "" || w.terminal {
		return nil
	}
	w.terminal = true
	if res := w.sink.Finish(); res.Tag() == witTypes.ResultErr {
		return writeError(res.Err())
	}
	return nil
}

func writeError(e streams.StreamWriteError) error {
	switch e.Tag() {
	case streams.StreamWriteErrorConcurrentOperation:
		return errors.New("golem: concurrent operation on the stdout stream")
	case streams.StreamWriteErrorClosed:
		cause := e.Closed()
		switch cause.Tag() {
		case streams.ByteStreamCloseCauseFinished:
			return errors.New("golem: stdout stream is already finished")
		case streams.ByteStreamCloseCauseConsumerCancelled:
			return errors.New("golem: the consumer cancelled the stdout stream")
		case streams.ByteStreamCloseCauseFailed:
			return &StreamError{Failure: StreamFailure{cause.Failed()}}
		}
	}
	return fmt.Errorf("golem: stdout stream write failed")
}
