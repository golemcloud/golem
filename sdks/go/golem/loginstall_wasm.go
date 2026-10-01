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

import golemlog "github.com/golemcloud/golem/sdks/go/golem/log"

// installDefaultLogger routes slog (and, via slog, the standard log package)
// through the host logging channel. Gated to the wasm target: on native builds
// the no-op variant is used so `go test` never links the wasi:logging host call.
func installDefaultLogger() { golemlog.SetDefault(nil) }
