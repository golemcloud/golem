package tool

import (
	"testing"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type panicReadSource struct{}

func (panicReadSource) Read([]streamItem) uint32 { panic("read panic") }
func (panicReadSource) WriterDropped() bool      { return false }

func TestStdinReadPanicStillReleasesReader(t *testing.T) {
	_, r, d := buildToolFor(t, declarePipe)
	e, input := pipeInput(t, r, "echo")
	drops := 0
	stdin := &byteReader{src: panicReadSource{}, release: func() { drops++ }}
	stdout := &fakeSink{}
	func() {
		defer func() {
			if got := recover(); got != "read panic" {
				t.Errorf("panic = %v, want read panic", got)
			}
		}()
		d.invokeCommand(e, nil, input, stdin, hostOutputs{stdout: stdout}, nil)
	}()
	if !stdout.dropped || stdout.failed == nil {
		t.Fatal("output cleanup did not run")
	}
	if drops != 1 {
		t.Fatalf("stdin released %d times after read unwound, want 1", drops)
	}
}

func TestStdinDropPreservesHandlerPanic(t *testing.T) {
	for _, dropPanics := range []bool{false, true} {
		t.Run(map[bool]string{false: "baseline", true: "drop_panics"}[dropPanics], func(t *testing.T) {
			original := &struct{ message string }{"handler panic"}
			_, r, d := buildToolFor(t, func(r *toolRegistry, d *definitions) {
				def := defineToolInto[Gate](r, d, "panic-cleanup", Spec{}, false)
				cmd := def.OutputBody[GateArgs, golem.Unit](func(a *GateArgs, s *CommandSpec) {
					s.Positional(&a.Mode)
					s.Stdout()
					s.Stderr()
				})
				_ = cmd.Handle(func(*OutputContext, GateArgs) (golem.Unit, error) { panic(original) })
			})
			e, _ := r.get("panic-cleanup")
			stdout, stderr := &fakeSink{}, &fakeSink{}
			drops := 0
			stdin := &byteReader{src: &fakeSource{}, release: func() {
				drops++
				if dropPanics {
					panic("stdin drop panic")
				}
			}}
			var got any
			func() {
				defer func() { got = recover() }()
				d.invokeCommand(e, nil, encodeArgs(t, e.root.body, func(*GateArgs) {}), stdin, hostOutputs{stdout, stderr}, nil)
			}()
			if drops != 1 || !stdout.dropped || !stderr.dropped || stdout.failed == nil || stderr.failed == nil {
				t.Fatal("handler did not run or cleanup did not execute")
			}
			if got != original {
				t.Fatalf("handler panic replaced: got %v, want original handler panic", got)
			}
		})
	}
}
