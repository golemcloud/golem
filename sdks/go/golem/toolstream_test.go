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
	"io"
	"strings"
	"testing"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	streams "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_streams"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

type streamItem = witTypes.Result[[]uint8, streams.ByteStreamFailure]

func chunk(s string) streamItem {
	return witTypes.Ok[[]uint8, streams.ByteStreamFailure]([]uint8(s))
}

func failure(f StreamFailure) streamItem {
	return witTypes.Err[[]uint8](f.wit)
}

// fakeSource replays a scripted sequence of stream items, then reports the
// writer as dropped — which is how a clean end of input arrives on the wire.
type fakeSource struct {
	items []streamItem
	at    int
}

func (f *fakeSource) Read(dst []streamItem) uint32 {
	if f.at >= len(f.items) {
		return 0
	}
	dst[0] = f.items[f.at]
	f.at++
	return 1
}

func (f *fakeSource) WriterDropped() bool { return f.at >= len(f.items) }

// fakeSink records what a handler wrote and which terminal it selected.
type fakeSink struct {
	written  []byte
	finished bool
	failed   *StreamFailure
	writeErr *streams.StreamWriteError
}

func (f *fakeSink) Write(b []uint8) witTypes.Result[witTypes.Unit, streams.StreamWriteError] {
	if f.writeErr != nil {
		return witTypes.Err[witTypes.Unit](*f.writeErr)
	}
	f.written = append(f.written, b...)
	return witTypes.Ok[witTypes.Unit, streams.StreamWriteError](witTypes.Unit{})
}

func (f *fakeSink) Finish() witTypes.Result[witTypes.Unit, streams.StreamWriteError] {
	f.finished = true
	return witTypes.Ok[witTypes.Unit, streams.StreamWriteError](witTypes.Unit{})
}

func (f *fakeSink) Fail(reason streams.ByteStreamFailure) witTypes.Result[witTypes.Unit, streams.StreamWriteError] {
	f.failed = &StreamFailure{reason}
	return witTypes.Ok[witTypes.Unit, streams.StreamWriteError](witTypes.Unit{})
}

func TestStdinReadsChunksAndEndsWithEOF(t *testing.T) {
	r := &ToolStdin{src: &fakeSource{items: []streamItem{chunk("hello, "), chunk("world")}}}
	got, err := io.ReadAll(r)
	if err != nil {
		t.Fatalf("ReadAll: %v", err)
	}
	if string(got) != "hello, world" {
		t.Errorf("read %q, want %q", got, "hello, world")
	}
}

// TestStdinSplitsChunksAcrossReads — the wire delivers whole chunks, but a
// caller's buffer may be smaller, so the remainder has to survive to the next
// Read rather than being dropped.
func TestStdinSplitsChunksAcrossReads(t *testing.T) {
	r := &ToolStdin{src: &fakeSource{items: []streamItem{chunk("abcdef")}}}
	buf := make([]byte, 4)

	n, err := r.Read(buf)
	if err != nil || n != 4 || string(buf[:n]) != "abcd" {
		t.Fatalf("first read: %q, %d, %v", buf[:n], n, err)
	}
	n, err = r.Read(buf)
	if err != nil || string(buf[:n]) != "ef" {
		t.Fatalf("second read: %q, %d, %v", buf[:n], n, err)
	}
	if _, err := r.Read(buf); !errors.Is(err, io.EOF) {
		t.Errorf("third read returned %v, want io.EOF", err)
	}
}

// TestStdinReportsAFailureItemAsAnError — a failure arrives as a stream value,
// so it must be distinguishable from the clean end of input.
func TestStdinReportsAFailureItemAsAnError(t *testing.T) {
	r := &ToolStdin{src: &fakeSource{items: []streamItem{
		chunk("partial"), failure(StreamResourceExhausted()),
	}}}
	_, err := io.ReadAll(r)
	if err == nil {
		t.Fatal("a failure item ended the stream cleanly")
	}
	var se *StreamError
	if !errors.As(err, &se) {
		t.Fatalf("error is %v, want a StreamError", err)
	}
	if se.Failure.String() != "resource exhausted" {
		t.Errorf("failure is %q, want resource exhausted", se.Failure)
	}
}

func TestStdinFailureReasonsRoundTrip(t *testing.T) {
	for _, tc := range []struct {
		f    StreamFailure
		want string
	}{
		{StreamCancelled(), "cancelled"},
		{StreamAbandoned(), "abandoned"},
		{StreamResourceExhausted(), "resource exhausted"},
		{StreamFailed("disk gone"), "failed: disk gone"},
	} {
		r := &ToolStdin{src: &fakeSource{items: []streamItem{failure(tc.f)}}}
		_, err := io.ReadAll(r)
		var se *StreamError
		if !errors.As(err, &se) {
			t.Fatalf("%s: error is %v, want a StreamError", tc.want, err)
		}
		if got := se.Failure.String(); got != tc.want {
			t.Errorf("failure rendered as %q, want %q", got, tc.want)
		}
	}
}

// TestAbsentStreamsExplainThemselves — a command invoked without a stream gets
// a reader and writer that say so, rather than a nil dereference.
func TestAbsentStreamsExplainThemselves(t *testing.T) {
	r := &ToolStdin{absent: absentStdin}
	if _, err := r.Read(make([]byte, 4)); err == nil || !strings.Contains(err.Error(), "without a stdin stream") {
		t.Errorf("reading an absent stdin gave %v", err)
	}
	w := &ToolStdout{absent: absentStdout}
	if _, err := w.Write([]byte("x")); err == nil || !strings.Contains(err.Error(), "without a stdout stream") {
		t.Errorf("writing an absent stdout gave %v", err)
	}
	// Finishing one that was never supplied is not an error: the dispatcher
	// finishes unconditionally.
	if err := w.finish(); err != nil {
		t.Errorf("finishing an absent stdout gave %v", err)
	}
}

func TestStdoutWritesAndFinishes(t *testing.T) {
	sink := &fakeSink{}
	w := &ToolStdout{sink: sink}
	if _, err := io.WriteString(w, "output"); err != nil {
		t.Fatalf("write: %v", err)
	}
	if err := w.finish(); err != nil {
		t.Fatalf("finish: %v", err)
	}
	if string(sink.written) != "output" || !sink.finished {
		t.Errorf("sink is %q finished=%v, want output/true", sink.written, sink.finished)
	}
}

// TestStdoutFirstTerminalWins — the wire accepts exactly one terminal, so a
// handler that failed the stream must not then have it finished underneath it.
func TestStdoutFirstTerminalWins(t *testing.T) {
	sink := &fakeSink{}
	w := &ToolStdout{sink: sink}
	if err := w.Fail(StreamCancelled()); err != nil {
		t.Fatalf("fail: %v", err)
	}
	if err := w.finish(); err != nil {
		t.Fatalf("finish after fail: %v", err)
	}
	if sink.finished {
		t.Error("finish overrode the failure terminal")
	}
	if sink.failed == nil || sink.failed.String() != "cancelled" {
		t.Errorf("failure is %v, want cancelled", sink.failed)
	}
	if _, err := w.Write([]byte("late")); err == nil {
		t.Error("writing after a terminal succeeded")
	}
}

func TestStdoutSurfacesWriteErrors(t *testing.T) {
	closed := streams.MakeStreamWriteErrorClosed(
		streams.MakeByteStreamCloseCauseFailed(StreamFailed("consumer died").wit))
	w := &ToolStdout{sink: &fakeSink{writeErr: &closed}}
	_, err := w.Write([]byte("x"))
	var se *StreamError
	if !errors.As(err, &se) {
		t.Fatalf("error is %v, want a StreamError", err)
	}
	if se.Failure.String() != "failed: consumer died" {
		t.Errorf("failure is %q", se.Failure)
	}

	concurrent := streams.MakeStreamWriteErrorConcurrentOperation()
	w = &ToolStdout{sink: &fakeSink{writeErr: &concurrent}}
	if _, err := w.Write([]byte("x")); err == nil || !strings.Contains(err.Error(), "concurrent operation") {
		t.Errorf("concurrent write gave %v", err)
	}
}

// invokeWithStreams runs a command against scripted streams, the way a host
// would for a body that declared both.
func invokeWithStreams(
	t *testing.T, d *definitions, e *toolEntry, input types.TypedSchemaValue,
	in []streamItem, sink *fakeSink,
) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
	t.Helper()
	return d.invokeCommand(e, nil, input,
		&ToolStdin{src: &fakeSource{items: in}}, &ToolStdout{sink: sink})
}

type PipeArgs struct {
	Mode Positional[string]
}

// declarePipe registers a command that copies stdin to stdout and reacts to the
// mode it is given, mirroring the tool-streaming test components.
func declarePipe(r *toolRegistry, d *definitions) {
	def := defineToolInto(r, d, "pipe", ToolSpec{Version: "0.1.0"})
	cmd := declareCommand[PipeArgs, uint64](r, d, def, nil, "", PipeArgs{}, []CommandOpt{
		Stdin(StreamSpec{Required: true}), Stdout(StreamSpec{Required: true}),
	})
	handleCommandInto(r, d, cmd, func(ctx *ToolContext, in PipeArgs) uint64 {
		switch in.Mode.Get() {
		case "resource-exhausted":
			Must0(ctx.Stdout().Fail(StreamResourceExhausted()))
			return 0
		case "panic":
			panic("handler gave up")
		}
		n, err := io.Copy(ctx.Stdout(), ctx.Stdin())
		if err != nil {
			panic(err)
		}
		return uint64(n)
	})
}

// TestCommandStreamsCopyAndFinish — returning from the handler selects the
// clean terminal, so an author never writes a finish call.
func TestCommandStreamsCopyAndFinish(t *testing.T) {
	_, r, d := buildToolFor(t, declarePipe)
	e, _ := r.get("pipe")
	sink := &fakeSink{}

	input := encodeToolArgs(t, d, e, nil, "echo")
	got := invokeWithStreams(t, d, e, input, []streamItem{chunk("abc"), chunk("de")}, sink)
	if got.Tag() != witTypes.ResultErr {
		if string(sink.written) != "abcde" {
			t.Errorf("stdout got %q, want abcde", sink.written)
		}
		if !sink.finished {
			t.Error("returning from the handler did not finish the stream")
		}
		if sink.failed != nil {
			t.Errorf("stream was failed with %v", sink.failed)
		}
	} else {
		t.Fatalf("invoke failed: %+v", got.Err())
	}

	typed := got.Ok().Result.Some()
	out, err := TypedValue{wit: typed}.JSON()
	if err != nil {
		t.Fatalf("result is not readable: %v", err)
	}
	// u64 travels as a canonical base-10 string.
	if out != "5" {
		t.Errorf("byte count %v, want \"5\"", out)
	}
}

// TestCommandStreamsRespectAnExplicitFailure — a handler that failed the stream
// itself keeps that terminal, and still returns a result.
func TestCommandStreamsRespectAnExplicitFailure(t *testing.T) {
	_, r, d := buildToolFor(t, declarePipe)
	e, _ := r.get("pipe")
	sink := &fakeSink{}

	input := encodeToolArgs(t, d, e, nil, "resource-exhausted")
	if got := invokeWithStreams(t, d, e, input, nil, sink); got.Tag() != witTypes.ResultOk {
		t.Fatalf("invoke failed: %+v", got.Err())
	}
	if sink.finished {
		t.Error("an explicitly failed stream was finished anyway")
	}
	if sink.failed == nil || sink.failed.String() != "resource exhausted" {
		t.Errorf("failure is %v, want resource exhausted", sink.failed)
	}
}

// TestCommandStreamsFailOnPanic — a dropped writer reads as `abandoned` on the
// wire, which would make a crashed handler look like a lost connection. The
// dispatcher fails the stream with the panic's message instead.
func TestCommandStreamsFailOnPanic(t *testing.T) {
	_, r, d := buildToolFor(t, declarePipe)
	e, _ := r.get("pipe")
	sink := &fakeSink{}

	input := encodeToolArgs(t, d, e, nil, "panic")
	func() {
		defer func() { _ = recover() }()
		invokeWithStreams(t, d, e, input, nil, sink)
	}()

	if sink.finished {
		t.Error("a panicking handler finished the stream cleanly")
	}
	if sink.failed == nil {
		t.Fatal("a panicking handler left the stream without a terminal")
	}
	if got := sink.failed.String(); got != "failed: handler gave up" {
		t.Errorf("failure is %q, want the panic message", got)
	}
}
