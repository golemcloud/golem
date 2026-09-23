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

package golem

import (
	mwExports "github.com/golemcloud/golem/sdks/go/golem/internal/exports/export_golem_tool_tool_middleware_guest"
	underlying "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_underlying"
)

// Off the wasm target there is no runtime to invoke, and binding the generated
// resource to the layer interface would drag its //go:wasmimport methods into a
// native link that has no bodies for them. The dispatcher and the handler
// protocol are target-independent and tested directly against a fake layer.

func newNextLayer(_ *underlying.UnderlyingTool) nextLayer { return absentNextLayer{} }

func newNextStdin(_ mwExports.Stdin) nextStdin { return nextStdin{} }
