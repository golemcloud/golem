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

// Package export_golem_api_save_snapshot holds the hand-written export slot for
// golem:api/save-snapshot@1.5.0 (an `async func` in WIT; the async lift lives in
// the generated glue). See export_golem_agent_guest for the pattern.
package export_golem_api_save_snapshot

import host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"

var Exports struct {
	Save func() host.Snapshot
}

func Save() host.Snapshot {
	if Exports.Save == nil {
		panic("golem: export save-snapshot was not registered — is the SDK imported?")
	}
	return Exports.Save()
}
