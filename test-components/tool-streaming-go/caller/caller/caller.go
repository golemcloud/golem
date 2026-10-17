// Package caller is the Go caller of the tool streaming matrix: it invokes
// whichever SDK's matrix-core provider the deployment binds, through the
// generated guest tool client.
package caller

import (
	"fmt"
	"io"
	"time"

	matrixcore "golem.local/bridge/matrix-core-tool-guest-client"
	stdinprobe "golem.local/bridge/stdin-probe-tool-guest-client"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type ID struct{ Name string }

var Caller = golem.DefineAgent[ID](golem.Spec{Name: "GoToolStreamingCaller"})

type Observation struct {
	Provider         string
	Command          string
	NormalizedSource string
	WeightedSize     int64
	LabelSummary     string
	Principal        string
	OwnerAgentId     string
	ErrorField       string
	ErrorReason      string
	ErrorRetryable   bool
}

var MatrixCoreObservation = Caller.Method[golem.Unit, Observation]("matrix_core_observation")

// StdinReleased calls a command that returns without reading its stdin, with
// a pipe nobody writes as that stdin, and reports whether the call closed the
// pipe once the tool stopped consuming it.
var StdinReleased = Caller.Method[golem.Unit, bool]("stdin_released")

// watchedPipe is a stdin source that reports its Close.
type watchedPipe struct {
	*io.PipeReader
	closed chan struct{}
}

func (p watchedPipe) Close() error {
	close(p.closed)
	return p.PipeReader.Close()
}

type state struct{}

func init() {
	impl := Caller.Implement(func(ID) *state { return &state{} })
	impl.Handle(StdinReleased, func(_ *golem.Context[state], _ golem.Unit) bool {
		r, w := io.Pipe()
		defer w.Close()
		src := watchedPipe{PipeReader: r, closed: make(chan struct{})}
		if _, err := stdinprobe.Ignore.Call(func(a *stdinprobe.IgnoreArgs) { a.Stdin = src }); err != nil {
			panic(fmt.Sprintf("stdin-probe ignore: %v", err))
		}
		select {
		case <-src.closed:
			return true
		case <-time.After(10 * time.Second):
			return false
		}
	})
	impl.Handle(MatrixCoreObservation, func(_ *golem.Context[state], _ golem.Unit) Observation {
		success, err := matrixcore.ArtifactInspect.Call(func(a *matrixcore.ArtifactInspectArgs) {
			a.Request = matrixcore.ArtifactInspectInRequest{
				Source:     "matrix.sample",
				Dimensions: matrixcore.Dimensions{Width: 3, Height: 5},
				Labels:     []string{"north", "east", "south"},
			}
			a.Multiplier = 7
		})
		if err != nil {
			panic(fmt.Sprintf("matrix-core inspect: %v", err))
		}
		_, err = matrixcore.ArtifactInspect.Call(func(a *matrixcore.ArtifactInspectArgs) {
			a.Request = matrixcore.ArtifactInspectInRequest{
				Source:     "reject.me",
				Dimensions: matrixcore.Dimensions{Width: 13, Height: 5},
				Labels:     []string{"unused", "error"},
			}
			a.Multiplier = 2
		})
		rejected, ok := matrixcore.ErrRejected.Match(err)
		if !ok {
			panic(fmt.Sprintf("matrix-core reject.me gave %v", err))
		}
		return Observation{
			Provider:         success.Provider,
			Command:          success.Command,
			NormalizedSource: success.NormalizedSource,
			WeightedSize:     success.WeightedSize,
			LabelSummary:     success.LabelSummary,
			Principal:        success.Principal,
			OwnerAgentId:     success.OwnerAgentID,
			ErrorField:       rejected.Field,
			ErrorReason:      rejected.Reason,
			ErrorRetryable:   rejected.Retryable,
		}
	})
}
