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
	"strings"
	"testing"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
)

// agentErrorToGo maps each host agent-error case onto a distinguishable Go error
// rather than flattening them into one string. Pure, so it is covered natively;
// ClientFor, which calls it around a host import, is covered by integration.
func TestAgentErrorToGoDistinguishesCases(t *testing.T) {
	cases := []struct {
		name string
		in   common.AgentError
		want string
	}{
		{"invalid input", common.MakeAgentErrorInvalidInput("bad field"), "invalid input: bad field"},
		{"invalid method", common.MakeAgentErrorInvalidMethod("no such method"), "invalid method: no such method"},
		{"invalid type", common.MakeAgentErrorInvalidType("no such type"), "invalid type: no such type"},
	}
	for _, tc := range cases {
		if got := agentErrorToGo(tc.in).Error(); got != tc.want {
			t.Errorf("%s: agentErrorToGo = %q, want %q", tc.name, got, tc.want)
		}
	}

	// The custom-error case carries a payload; it stays distinguishable rather
	// than collapsing into one of the above.
	if got := agentErrorToGo(customError("boom")).Error(); !strings.HasPrefix(got, "custom error:") {
		t.Errorf("custom error mapped to %q, want a \"custom error:\" prefix", got)
	}
}
