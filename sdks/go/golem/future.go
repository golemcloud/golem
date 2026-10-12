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

	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
)

// Future is an invocation that has been started but not yet awaited, returned by
// [MethodDef.CallAsync].
//
// Several futures can be in flight at once. Get is asynchronous: a goroutine
// blocked in it yields to the component-model event loop and lets other
// goroutines run. [MethodDef.Call] is CallAsync followed by Get, so it yields in
// the same way.
//
// It wraps the host resource rather than replacing it — a handle, not data, so
// nothing here crosses the schema marshaler.
type Future[Out any] struct {
	// ID identifies the invocation, available before the result is.
	ID InvocationID

	fut    *host.FutureInvokeResult
	method string
	target string
	done   bool
	out    Out
	err    error
}

// ErrCallCancelled is what getting the result of a cancelled call reports.
// Cancelling only stops waiting: the call may already have run.
var ErrCallCancelled = errors.New("golem: the call was cancelled before its result was awaited")

// Get waits for the invocation to finish and returns its result. A remote
// failure, or a result that does not decode as Out, is returned as a
// [RemoteCallError]. Model expected outcomes as a [Result] in Out.
//
// The result is kept: a later Get returns it again. After [Future.Cancel], Get
// returns [ErrCallCancelled].
func (f *Future[Out]) Get() (Out, error) {
	if f == nil || (f.fut == nil && !f.done) {
		panic(fmt.Errorf("golem: Get on a zero Future"))
	}
	if f.done {
		return f.out, f.err
	}
	res := f.fut.Get()
	f.fut.Drop()
	f.fut = nil
	f.done = true
	if res.IsErr() {
		f.err = rpcErrorToGo(f.target, f.method, res.Err())
	} else {
		f.out, f.err = decodeOutput[Out](f.target, f.method, res.Ok())
	}
	return f.out, f.err
}

// MustGet is [Future.Get] that panics on a remote failure.
func (f *Future[Out]) MustGet() Out {
	out, err := f.Get()
	if err != nil {
		panic(err)
	}
	return out
}

// Cancel makes a best-effort attempt to cancel the invocation; a later Get
// reports [ErrCallCancelled]. It does nothing once the result was taken.
func (f *Future[Out]) Cancel() {
	if f == nil || f.done || f.fut == nil {
		return
	}
	f.fut.Cancel()
	f.fut.Drop()
	f.fut = nil
	f.done = true
	f.err = ErrCallCancelled
}
