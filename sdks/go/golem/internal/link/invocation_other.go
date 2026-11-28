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

package link

import (
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Off the wasm target there is no host to hold an invocation, and binding the
// generated resource here would drag its //go:wasmimport declarations into a
// native link that has no bodies for them.

// HostInvocationOps is only available inside a component.
func HostInvocationOps(*host.Invocation) InvocationOps {
	panic("golem: invocation handles are only available inside a component")
}

// GetInvocationOps is only available inside a component.
func GetInvocationOps(types.InvocationId) InvocationOps {
	panic("golem: invocation handles are only available inside a component")
}
