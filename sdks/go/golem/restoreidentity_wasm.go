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
	"errors"

	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// parseRestoreIdentity asks the host to split an agent id string into the agent
// type name and its constructor parameters — what initialize needs to bring the
// agent back during snapshot recovery. Wasm-only: it is a host call.
func parseRestoreIdentity(agentID string) (string, types.SchemaValueTree, error) {
	res := host.ParseAgentId(agentID)
	if res.IsErr() {
		return "", types.SchemaValueTree{}, errors.New(agentErrorToGo(res.Err()).Error())
	}
	t := res.Ok()
	return t.F0, t.F1.Value, nil
}
