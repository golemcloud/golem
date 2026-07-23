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

// Package export_golem_api_load_snapshot holds the hand-written export slot for
// golem:api/load-snapshot@1.5.0. See export_golem_agent_guest for the pattern.
package export_golem_api_load_snapshot

import (
	host "github.com/golemcloud/golem/sdks/go/internal/wit/golem_api_host"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

var Exports struct {
	Load func(snapshot host.Snapshot) witTypes.Result[witTypes.Unit, string]
}

func Load(snapshot host.Snapshot) witTypes.Result[witTypes.Unit, string] {
	if Exports.Load == nil {
		panic("golem: export load-snapshot was not registered — is the SDK imported?")
	}
	return Exports.Load(snapshot)
}
