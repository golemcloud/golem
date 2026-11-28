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

package link

import (
	"runtime"

	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// hostInvocation owns an invocation resource, which it drops when the
// operations that reach it are collected. Dropping it only releases the
// handle: the invocation keeps running.
type hostInvocation struct{ raw *host.Invocation }

// HostInvocationOps are the operations of an invocation resource, which they
// take ownership of.
func HostInvocationOps(raw *host.Invocation) InvocationOps {
	h := &hostInvocation{raw: raw}
	runtime.AddCleanup(h, func(r *host.Invocation) { r.Drop() }, raw)
	return InvocationOps{
		ID:     raw.Id(),
		Status: func() witTypes.Result[host.InvocationStatus, host.RpcError] { return h.raw.Status() },
		Await: func() witTypes.Result[witTypes.Option[types.SchemaValueTree], host.RpcError] {
			return h.raw.Await()
		},
		Cancel: func() witTypes.Result[bool, host.RpcError] { return h.raw.Cancel() },
	}
}

// GetInvocationOps are the operations of the invocation with the ID. Building
// them makes no remote call.
func GetInvocationOps(id types.InvocationId) InvocationOps {
	return HostInvocationOps(host.InvocationGet(id))
}
