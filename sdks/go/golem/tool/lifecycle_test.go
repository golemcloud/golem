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
	"io"
	"reflect"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/golemcloud/golem/sdks/go/golem"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	streams "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_streams"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

type blockedSource struct{ entered, settle chan struct{} }

func (s blockedSource) Read(dst []witTypes.Result[[]uint8, streams.ByteStreamFailure]) uint32 {
	close(s.entered)
	<-s.settle
	dst[0] = witTypes.Ok[[]uint8, streams.ByteStreamFailure]([]byte("late"))
	return 1
}
func (blockedSource) WriterDropped() bool { return false }

func TestCloseDuringReadIsPermanentAndIndependent(t *testing.T) {
	s := blockedSource{make(chan struct{}), make(chan struct{})}
	var drops, cancels atomic.Int32
	stdout := &byteReader{src: s, release: func() { drops.Add(1) }}
	stderr := &byteReader{src: &fakeSource{items: []streamItem{chunk("sibling")}}}
	i := newInvocation(toolCall{stdout: stdout, stderr: stderr, cancel: func() { cancels.Add(1) }, wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
		return witTypes.None[types.TypedSchemaValue](), nil
	}}, func(toolCall) (int, error) { return 71, nil })
	r := i.Stdout()
	done := make(chan struct{})
	go func() {
		defer close(done)
		n, err := r.Read(make([]byte, 8))
		if n != 0 || !errors.Is(err, io.EOF) {
			t.Errorf("late read = %d, %v", n, err)
		}
	}()
	<-s.entered
	if err := r.Close(); err != nil {
		t.Fatal(err)
	}
	if drops.Load() != 0 || cancels.Load() != 0 {
		t.Fatal("close interrupted read or invocation")
	}
	bytes, err := io.ReadAll(i.Stderr())
	if string(bytes) != "sibling" || err != nil {
		t.Fatalf("sibling %q, %v", bytes, err)
	}
	close(s.settle)
	<-done
	if drops.Load() != 1 {
		t.Fatal("reader not released once")
	}
	if n, err := r.Read(make([]byte, 8)); n != 0 || !errors.Is(err, io.EOF) {
		t.Fatal("closed reader resumed")
	}
	if out, err := i.Wait(); out != 71 || err != nil {
		t.Fatalf("result %d, %v", out, err)
	}
}

func TestInvocationConcurrentWaitCancelDetach(t *testing.T) {
	for _, detach := range []bool{false, true} {
		entered, settle := make(chan struct{}), make(chan struct{})
		var gets, drops, cancels, decodes, disposes atomic.Int32
		call := guardCancel(toolCall{
			wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
				gets.Add(1)
				close(entered)
				<-settle
				return witTypes.Some(types.TypedSchemaValue{}), nil
			},
			drop: func() { drops.Add(1) }, cancel: func() {
				if drops.Load() != 0 {
					t.Error("cancel after physical drop")
				}
				cancels.Add(1)
			},
		})
		i := newInvocation(call, func(toolCall) (int, error) { decodes.Add(1); return 83, nil })
		i.result.Dispose = func(r toolOutcome) {
			if r.result.IsNone() {
				t.Error("lost raw result")
			}
			disposes.Add(1)
		}
		var wg sync.WaitGroup
		for range 12 {
			wg.Add(1)
			go func() {
				defer wg.Done()
				out, err := i.Wait()
				if detach {
					if out != 0 || !errors.Is(err, ErrResultDetached) {
						t.Errorf("detached = %d, %v", out, err)
					}
				} else if out != 83 || err != nil {
					t.Errorf("result = %d, %v", out, err)
				}
			}()
		}
		<-entered
		i.Cancel()
		if detach {
			i.Detach()
			if drops.Load() != 0 {
				t.Fatal("drop during Get")
			}
		}
		for range 12 {
			wg.Add(1)
			go func() { defer wg.Done(); i.Cancel() }()
		}
		close(settle)
		wg.Wait()
		i.Cancel()
		if gets.Load() != 1 || drops.Load() != 1 || cancels.Load() == 0 {
			t.Fatalf("gets=%d drops=%d cancels=%d", gets.Load(), drops.Load(), cancels.Load())
		}
		if detach {
			if decodes.Load() != 0 || disposes.Load() != 1 {
				t.Fatal("late result decoded or leaked")
			}
		} else if decodes.Load() != 1 || disposes.Load() != 0 {
			t.Fatal("delivered result ownership changed")
		}
	}
}

func TestDetachBeforeWaitDoesNotCancelOrCloseOutputs(t *testing.T) {
	var drops int
	i := newInvocation(guardCancel(toolCall{
		stdout: &byteReader{src: &fakeSource{items: []streamItem{chunk("still running")}}},
		wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
			t.Fatal("detached Get reached host")
			return witTypes.None[types.TypedSchemaValue](), nil
		},
		cancel: func() { t.Fatal("detach cancelled invocation") }, drop: func() { drops++ },
	}), func(toolCall) (int, error) { t.Fatal("detached result decoded"); return 0, nil })
	i.Detach()
	i.Detach()
	i.Cancel()
	stdout := i.Stdout()
	if _, err := i.Wait(); !errors.Is(err, ErrResultDetached) {
		t.Fatal(err)
	}
	bytes, err := io.ReadAll(stdout)
	if string(bytes) != "still running" || err != nil || drops != 1 {
		t.Fatalf("output=%q err=%v drops=%d", bytes, err, drops)
	}
}

type gatedOutputSource struct {
	fakeSource
	entered, settle chan struct{}
}

func (s *gatedOutputSource) Read(dst []streamItem) uint32 {
	if s.at == 0 {
		close(s.entered)
		<-s.settle
	}
	return s.fakeSource.Read(dst)
}

func TestDetachedWaitDrainsUntakenOutputs(t *testing.T) {
	for _, middleware := range []bool{false, true} {
		t.Run(map[bool]string{false: "discard", true: "forward"}[middleware], func(t *testing.T) {
			settle := make(chan struct{})
			unblock := sync.OnceFunc(func() { close(settle) })
			defer unblock()
			stdout := &gatedOutputSource{fakeSource{items: []streamItem{chunk("stdout bytes")}}, make(chan struct{}), settle}
			stderr := &gatedOutputSource{fakeSource{items: []streamItem{chunk("stderr bytes")}}, make(chan struct{}), settle}
			stdoutDone, stderrDone := make(chan struct{}), make(chan struct{})
			i := newInvocation(guardCancel(toolCall{
				stdout: &byteReader{src: stdout, release: func() { close(stdoutDone) }},
				stderr: &byteReader{src: stderr, release: func() { close(stderrDone) }},
				wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
					t.Error("detached Wait observed the result")
					return witTypes.None[types.TypedSchemaValue](), nil
				},
				cancel: func() { t.Error("detached Wait cancelled invocation") },
			}), func(toolCall) (int, error) { t.Error("detached result decoded"); return 0, nil })
			outSink, errSink := &fakeSink{}, &fakeSink{}
			if middleware {
				i.passThrough = &middlewareInvocation{stdout: newToolOutput("stdout", outSink), stderr: newToolOutput("stderr", errSink)}
			}
			i.Detach()
			var wg sync.WaitGroup
			for range 12 {
				wg.Go(func() {
					if _, err := i.Wait(); !errors.Is(err, ErrResultDetached) {
						t.Errorf("Wait = %v", err)
					}
				})
			}
			waited := make(chan struct{})
			go func() { wg.Wait(); close(waited) }()
			for _, done := range []chan struct{}{waited, stdout.entered, stderr.entered} {
				select {
				case <-done:
				case <-time.After(time.Second):
					t.Fatal("detached Wait blocked or did not start both output drains")
				}
			}
			unblock()
			for _, done := range []chan struct{}{stdoutDone, stderrDone} {
				select {
				case <-done:
				case <-time.After(time.Second):
					t.Fatal("untaken output was not drained and released")
				}
			}
			if middleware && (string(outSink.written) != "stdout bytes" || string(errSink.written) != "stderr bytes") {
				t.Fatal("detached Wait did not forward both outputs")
			}
			if _, err := io.ReadAll(i.Stdout()); !errors.Is(err, ErrOutputDrained) {
				t.Fatalf("stdout after drain = %v", err)
			}
		})
	}
}

func TestClosedMiddlewareOutputKeepsSiblingAndResult(t *testing.T) {
	for index, cause := range []streams.ByteStreamCloseCause{streams.MakeByteStreamCloseCauseConsumerCancelled(), streams.MakeByteStreamCloseCauseFinished(), streams.MakeByteStreamCloseCauseFailed(OutputFailed("explicit").wit), streams.MakeByteStreamCloseCauseFinished()} {
		closed := streams.MakeStreamWriteErrorClosed(cause)
		stdout, stderr := &fakeSink{writeErr: &closed}, &fakeSink{}
		var releases int
		i := newInvocation(guardCancel(toolCall{
			stdout: &byteReader{src: &fakeSource{items: []streamItem{chunk("closed bytes")}}, release: func() { releases++ }},
			stderr: &byteReader{src: &fakeSource{items: []streamItem{chunk("kept bytes")}}},
			wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
				return witTypes.None[types.TypedSchemaValue](), nil
			},
			cancel: func() { t.Error("output closure cancelled invocation") },
		}), func(toolCall) (int, error) { return 41, nil })
		i.passThrough = &middlewareInvocation{stdout: newToolOutput("stdout", stdout), stderr: newToolOutput("stderr", stderr)}
		if index == 3 {
			if err := i.passThrough.stdout.Fail(OutputFailed("explicit local")); err != nil {
				t.Fatal(err)
			}
		}
		if out, err := i.Wait(); out != 41 || err != nil {
			t.Fatalf("result %d, %v", out, err)
		}
		if releases != 1 || string(stderr.written) != "kept bytes" || len(stdout.written) != 0 {
			t.Fatal("closure affected sibling or lost cleanup")
		}
		if index == 3 && (stdout.failed == nil || stdout.failed.String() != "failed: explicit local") {
			t.Fatal("explicit terminal replaced")
		}
	}
}

type unprintableError struct{}

func (*unprintableError) Error() string { panic("formatting panic") }

func TestPanicFormattingCannotSkipCleanup(t *testing.T) {
	original := &unprintableError{}
	stdout, stderr := &fakeSink{}, &fakeSink{}
	func() {
		defer func() {
			if got := recover(); got != original {
				t.Errorf("original panic replaced: %T", got)
			}
		}()
		_, _ = runWithOutputs("test", []*Output{newToolOutput("stdout", stdout), newToolOutput("stderr", stderr)}, func() (reflect.Value, error) { panic(original) })
	}()
	if !stdout.dropped || !stderr.dropped || stdout.failed == nil || stderr.failed == nil {
		t.Fatal("panic skipped outputs")
	}
}

func TestUniversalMiddlewareUnknownRaisedOutcomeIsInvalidResult(t *testing.T) {
	for _, panicked := range []bool{false, true} {
		v, r, d := newVcs(t)
		audit := defineUniversalToolMiddlewareInto[golem.Unit](r, d, "raised-audit", MiddlewareSpec{})
		_ = audit.Handle(func(*UniversalMiddlewareContext[golem.Unit]) (golem.Option[golem.TypedValue], error) {
			err := v.errNothing.New(golem.Unit{})
			if panicked {
				panic(err)
			}
			return golem.None[golem.TypedValue](), err
		})
		e, _ := r.get("vcs")
		res, sink := runMiddlewareFor(t, r, d, "raised-audit", middlewareRun{path: []string{"commit"}, input: encodeArgs(t, v.commit.ce, func(a *CommitArgs) { a.Message = "m" }), params: mustTypedValue(t, golem.Unit{}), under: localUnderlying(d, e)})
		if !res.IsErr() || res.Err().Tag() != types.ToolErrorInvalidResult {
			t.Fatal("unknown raised outcome did not become invalid-result")
		}
		if !sink.finished || !sink.dropped {
			t.Fatal("unknown raised outcome did not finish output")
		}
	}
}

type lifecycleEnum uint8

func TestMalformedReturnedOutcomeFinishesOutputs(t *testing.T) {
	_, r, d := buildToolFor(t, func(r *toolRegistry, d *definitions) {
		d.DefineEnum(reflect.TypeFor[lifecycleEnum](), []string{"valid"})
		def := defineToolInto[Gate](r, d, "malformed", Spec{}, false)
		cmd := def.OutputBody[GateArgs, lifecycleEnum](func(a *GateArgs, s *CommandSpec) { s.Positional(&a.Mode); s.Stdout(); s.Stderr() })
		_ = cmd.Handle(func(ctx *OutputContext, _ GateArgs) (lifecycleEnum, error) {
			_, _ = io.WriteString(ctx.Stdout(), "bytes")
			return 5, nil
		})
	})
	e, _ := r.get("malformed")
	stdout, stderr := &fakeSink{}, &fakeSink{}
	res := d.invokeCommand(e, nil, encodeArgs(t, e.root.body, func(a *GateArgs) {}), &byteReader{absent: absentStdin}, hostOutputs{stdout, stderr}, nil)
	if !res.IsErr() || res.Err().Tag() != types.ToolErrorInvalidResult {
		t.Fatal("malformed return did not become invalid-result")
	}
	if string(stdout.written) != "bytes" || !stdout.finished || !stderr.finished || !stdout.dropped || !stderr.dropped {
		t.Fatal("invalid return changed output cleanup")
	}
}

func TestToolUnitResultRejectsUnexpectedPayloadAndPreservesDeclaredErrors(t *testing.T) {
	def := defineToolInto[Gate](newToolRegistry(), newDefinitions(), "unit-result", Spec{}, true)
	plain := def.Command[golem.Unit, golem.Unit]("plain", nil)
	streamed := def.OutputCommand[golem.Unit, golem.Unit]("streamed", func(_ *golem.Unit, s *CommandSpec) { s.Stdout() })
	payload := types.TypedSchemaValue{Value: types.SchemaValueTree{Root: 0, ValueNodes: []types.SchemaValueNode{
		types.MakeSchemaValueNodeSecretValue(new(types.Secret)),
	}}}
	for _, declaredError := range []bool{false, true} {
		t.Run(map[bool]string{false: "unexpected-success-payload", true: "declared-error-payload"}[declaredError], func(t *testing.T) {
			previous := startToolCall
			t.Cleanup(func() { startToolCall = previous })
			gets, drops := 0, 0
			startToolCall = func(string, []string, types.TypedSchemaValue, io.Reader, Streams) (toolCall, error) {
				return guardCancel(toolCall{
					wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
						gets++
						if declaredError {
							err := types.MakeToolRpcErrorRemoteToolError(types.MakeToolErrorCustomError(types.CustomToolError{Name: "expected", Payload: payload}))
							return witTypes.None[types.TypedSchemaValue](), &err
						}
						return witTypes.Some(payload), nil
					},
					drop: func() { drops++ },
				}), nil
			}
			check := func(err error) {
				t.Helper()
				var ce *CallError
				if !errors.As(err, &ce) {
					t.Fatalf("expected CallError, got %v", err)
				}
				if declaredError {
					if ce.Kind != CallDeclaredError || ce.ErrorName != "expected" || !reflect.DeepEqual(witOf(ce.Payload()), payload) {
						t.Fatalf("declared error payload changed: %v", ce)
					}
				} else if ce.Kind != CallInvalidResult {
					t.Fatalf("unexpected Unit payload accepted: %v", ce)
				}
			}
			_, err := plain.Call(nil)
			check(err)
			inv, err := streamed.Call(nil)
			if err != nil {
				t.Fatal(err)
			}
			_, first := inv.Wait()
			check(first)
			_, again := inv.Wait()
			if first != again {
				t.Fatal("repeated Wait did not preserve the delivered error")
			}
			inv.Detach()
			check(first)
			if gets != 2 || drops != 2 {
				t.Fatalf("result resources: gets %d, drops %d", gets, drops)
			}
		})
	}
}

func TestInvocationDropPanicPreservesWaitPanic(t *testing.T) {
	i := newInvocation(guardCancel(toolCall{
		wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) { panic("original") },
		drop: func() { panic("drop") },
	}), func(toolCall) (int, error) { return 0, nil })
	var wg sync.WaitGroup
	for range 4 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			defer func() {
				if recover() != "original" {
					t.Error("drop masked original")
				}
			}()
			_, _ = i.Wait()
		}()
	}
	wg.Wait()
}

type blockedEOFSource struct{ entered, settle chan struct{} }

func (s blockedEOFSource) Read([]witTypes.Result[[]uint8, streams.ByteStreamFailure]) uint32 {
	close(s.entered)
	<-s.settle
	return 0
}
func (blockedEOFSource) WriterDropped() bool { return true }

func TestDetachReleasesCachedRawResultWithoutWaitingForOutput(t *testing.T) {
	s := blockedEOFSource{make(chan struct{}), make(chan struct{})}
	ready, disposed, done := make(chan struct{}), make(chan struct{}), make(chan struct{})
	i := newInvocation(guardCancel(toolCall{
		stdout: &byteReader{src: s},
		wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
			close(ready)
			return witTypes.Some(types.TypedSchemaValue{}), nil
		},
	}), func(toolCall) (int, error) { t.Error("detached raw result decoded"); return 0, nil })
	i.result.Dispose = func(toolOutcome) { close(disposed) }
	go func() {
		defer close(done)
		_, err := i.Wait()
		if !errors.Is(err, ErrResultDetached) {
			t.Errorf("Wait = %v", err)
		}
	}()
	<-ready
	<-s.entered
	i.Detach()
	select {
	case <-disposed:
	case <-time.After(time.Second):
		t.Error("cached raw result retained by independent output")
	}
	close(s.settle)
	<-done
}

type blockedSink struct {
	fakeSink
	entered, settle chan struct{}
	active          atomic.Bool
	overlapped      atomic.Bool
}

func (s *blockedSink) Write(b []byte) witTypes.Result[witTypes.Unit, streams.StreamWriteError] {
	s.active.Store(true)
	close(s.entered)
	<-s.settle
	s.active.Store(false)
	return s.fakeSink.Write(b)
}
func (s *blockedSink) Finish() witTypes.Result[witTypes.Unit, streams.StreamWriteError] {
	if s.active.Load() {
		s.overlapped.Store(true)
	}
	return s.fakeSink.Finish()
}
func (s *blockedSink) release() {
	if s.active.Load() {
		s.overlapped.Store(true)
	}
	s.fakeSink.release()
}

func TestDetachedMiddlewareDoesNotDropActiveWriter(t *testing.T) {
	sink := &blockedSink{entered: make(chan struct{}), settle: make(chan struct{})}
	ready, disposed, handlerDone, cleanupStarted := make(chan struct{}), make(chan struct{}), make(chan struct{}), make(chan struct{})
	i := newInvocation(guardCancel(toolCall{
		stdout: &byteReader{src: &fakeSource{items: []streamItem{chunk("kept")}}},
		wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
			close(ready)
			return witTypes.Some(types.TypedSchemaValue{}), nil
		},
	}), func(toolCall) (int, error) { t.Error("detached result decoded"); return 0, nil })
	output := newToolOutput("stdout", sink)
	i.passThrough = &middlewareInvocation{stdout: output}
	i.result.Dispose = func(toolOutcome) { close(disposed) }
	go func() {
		defer close(handlerDone)
		_, _ = runWithOutputs("middleware", []*Output{output}, func() (reflect.Value, error) {
			_, err := i.Wait()
			if !errors.Is(err, ErrResultDetached) {
				t.Errorf("Wait = %v", err)
			}
			close(cleanupStarted)
			return reflect.ValueOf(31), nil
		})
	}()
	<-ready
	<-sink.entered
	i.Detach()
	<-disposed
	<-cleanupStarted
	select {
	case <-handlerDone:
		t.Error("cleanup completed during active write")
	case <-time.After(10 * time.Millisecond):
	}
	close(sink.settle)
	<-handlerDone
	if sink.overlapped.Load() || !sink.dropped || !sink.finished || string(sink.written) != "kept" {
		t.Fatal("writer cleanup overlapped write or lost bytes/terminal")
	}
}
