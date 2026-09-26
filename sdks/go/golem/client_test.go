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
	"errors"
	"testing"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
)

// TestAgentErrorToGoDistinguishesCases — agentErrorToGo maps each host agent-error case onto a distinguishable Go error
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

	// The custom-error case carries a string payload; it must round-trip intact
	// (not render as a struct dump — see customErrorMessage).
	if got := agentErrorToGo(customError("boom")).Error(); got != "custom error: boom" {
		t.Errorf("custom error mapped to %q, want %q", got, "custom error: boom")
	}
}

// TestAgentErrorIsRecoverable — the kind is inspectable via errors.As.
func TestAgentErrorIsRecoverable(t *testing.T) {
	err := agentErrorToGo(customError("boom"))
	var ae *AgentError
	if !errors.As(err, &ae) {
		t.Fatalf("agentErrorToGo did not return an *AgentError: %T", err)
	}
	if ae.Kind != AgentCustom || ae.Message != "boom" {
		t.Fatalf("AgentError = {Kind:%d Message:%q}, want {Custom, boom}", ae.Kind, ae.Message)
	}
	if got := agentErrorToGo(common.MakeAgentErrorInvalidMethod("nope")); got.(*AgentError).Kind != AgentInvalidMethod {
		t.Fatalf("invalid-method kind = %d", got.(*AgentError).Kind)
	}
}
