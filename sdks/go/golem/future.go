// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package golem

import (
	"fmt"

	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
)

// Future is an invocation that has been started but not yet awaited, returned by
// [MethodDef.CallAsync].
//
// Several futures can be in flight at once. This is the SDK's only source of
// concurrency: `invoke-and-await` is a synchronous import that blocks the whole
// component, while a future's Get is async, so a goroutine blocked in Get yields
// to the component-model event loop and lets other goroutines run.
//
// It wraps the host resource rather than replacing it — a handle, not data, so
// nothing here crosses the schema marshaler.
type Future[Out any] struct {
	// ID identifies the invocation, available before the result is.
	ID InvocationID

	fut    *host.FutureInvokeResult
	method string
	target string
}

// Get waits for the invocation to finish and decodes its result.
//
// It consumes the future: the underlying host handle is owned, so it is dropped
// here and a second call reports an error rather than reusing a freed handle.
func (f *Future[Out]) Get() (Out, error) {
	var zero Out
	if f == nil || f.fut == nil {
		return zero, fmt.Errorf("golem: Get on an already-consumed or zero Future")
	}

	res := f.fut.Get()
	f.fut.Drop()
	f.fut = nil

	if res.IsErr() {
		return zero, rpcErrorToGo(f.target, f.method, res.Err())
	}
	return decodeOutput[Out](f.target, f.method, res.Ok())
}

// Cancel makes a best-effort attempt to cancel the invocation. It is a no-op if
// the invocation already started or finished, and it consumes the future.
func (f *Future[Out]) Cancel() {
	if f == nil || f.fut == nil {
		return
	}
	f.fut.Cancel()
	f.fut.Drop()
	f.fut = nil
}
