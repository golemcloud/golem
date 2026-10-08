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
	"os"
	"runtime/debug"
)

// An unrecovered panic makes the Go runtime exit with status 2, which Golem
// records as a deliberate process exit. At the crash level the runtime aborts
// through a wasm trap instead, which Golem treats as a failure: it fails the
// agent, or retries the invocation inside an atomic region, as for the other
// SDKs. This covers panics on any goroutine.
func init() { debug.SetTraceback("crash") }

// trap reports an agent method's panic and traps the component. The report is
// one write, so it is one log entry; the runtime's own panic report writes each
// token separately and dumps every goroutine.
func trap(pe *PanicError) {
	_, _ = os.Stderr.WriteString("panic: " + pe.Error() + "\n\n" + string(debug.Stack()))
	abort()
}

// abort executes the wasm unreachable instruction.
func abort()
