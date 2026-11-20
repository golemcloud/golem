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

package golem

import (
	"encoding/json"
	"fmt"
)

// A snapshot written by a 1.6 SDK carries the 1.6 principal in the Go form:
// {"oidc":{…}}, {"agent":{…}}, {"golemUser":{…}}, or {} for anonymous. A 2.0
// agent reads such a snapshot as strictly as before (a malformed principal
// still fails the load) and drops the principal: the caller of an invocation
// comes from the host, never from saved state.
type legacyPrincipal struct {
	Oidc *struct {
		Sub    string `json:"sub"`
		Issuer string `json:"issuer"`
		Claims string `json:"claims"`
	} `json:"oidc,omitempty"`
	Agent *struct {
		AgentID AgentID `json:"AgentID"`
	} `json:"agent,omitempty"`
	GolemUser *struct {
		AccountID UUID `json:"AccountID"`
	} `json:"golemUser,omitempty"`
}

// validateLegacyPrincipal checks the principal of a 1.6 snapshot and discards it.
func validateLegacyPrincipal(data []byte) error {
	var dto legacyPrincipal
	if err := json.Unmarshal(data, &dto); err != nil {
		return fmt.Errorf("golem: decoding the snapshot's principal: %w", err)
	}
	return nil
}
