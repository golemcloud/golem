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

//go:build !wasip1

package tool

import (
	toolExports "github.com/golemcloud/golem/sdks/go/golem/internal/exports/export_golem_tool_guest"
)

// Off the wasm target the host never supplies a stream, and binding the
// generated reader and writer to the stream interfaces would drag their
// //go:wasmimport methods into a native link, which has no bodies for them. The
// adapters themselves are target-independent and tested directly; only these
// two functions are not (see toolstream_wasm.go).

func newToolStdin(_ toolExports.Stdin) *byteReader {
	return &byteReader{absent: absentStdin}
}

func hostOutput(_ toolExports.Stdout) byteStreamSink { return nil }
