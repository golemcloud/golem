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
	"strings"
	"testing"

	"github.com/golemcloud/golem/sdks/go/golem/internal/identity"
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// TestAgentErrorToGoDistinguishesCases — agentErrorToGo maps each host agent-error case onto a distinguishable Go error
// rather than flattening them into one string. Pure, so it is covered natively;
// the client constructors, which call it around a host import, are covered by
// integration.
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
	if ae.Kind != AgentCustomError || ae.Message != "boom" {
		t.Fatalf("AgentError = {Kind:%d Message:%q}, want {Custom, boom}", ae.Kind, ae.Message)
	}
	if got := agentErrorToGo(common.MakeAgentErrorInvalidMethod("nope")); got.(*AgentError).Kind != AgentInvalidMethod {
		t.Fatalf("invalid-method kind = %d", got.(*AgentError).Kind)
	}
}

// TestCreateOptionsReachTheHost — Keyed, the checked configuration overrides
// and the environment variables become the host's create options; nothing else
// is added, and the caller's environment is not copied.
func TestCreateOptionsReachTheHost(t *testing.T) {
	withDefs(t, func(d *definitions) {
		cfgConfiguredAgent[demoAppConfig](d)
		noDefErrs(t, d)
		e := d.agents["Cfg"]
		t.Setenv("CALLER_ONLY", "x")
		for _, keyed := range []bool{false, true} {
			opts := []CreateOpt{withConfig(demoAppConfig{Greeting: "hi"}), WithEnv("MODE", "test"), WithEnv("LEVEL", "2")}
			if keyed {
				opts = append(opts, Keyed())
			}
			options, err := createOptions(d, e, opts)
			if err != nil {
				t.Fatal(err)
			}
			if options.Keyed != keyed {
				t.Errorf("keyed = %v, want %v", options.Keyed, keyed)
			}
			paths := map[string]bool{}
			for _, c := range options.AgentConfig {
				paths[strings.Join(c.Path, "/")] = true
			}
			if !paths["greeting"] || !paths["db/url"] || paths["db/password"] {
				t.Errorf("config overrides = %v", paths)
			}
			if len(options.Env) != 2 || options.Env[0].F0 != "MODE" || options.Env[0].F1 != "test" || options.Env[1].F0 != "LEVEL" {
				t.Errorf("env = %+v", options.Env)
			}
		}
		options, err := createOptions(d, e, nil)
		if err != nil || options.Keyed || options.AgentConfig != nil || options.Env != nil {
			t.Errorf("no options gave %+v, %v", options, err)
		}
	})
}

// TestCreateRejectsAnUndeclaredOverride — an override that the target does not
// declare is refused before the host is reached.
func TestCreateRejectsAnUndeclaredOverride(t *testing.T) {
	withDefs(t, func(d *definitions) {
		cfgConfiguredAgent[NoConfig](d)
		_, err := createOptions(d, d.agents["Cfg"], []CreateOpt{withConfig(demoAppConfig{Greeting: "x"})})
		if err == nil || !strings.Contains(err.Error(), "not a declared config key") {
			t.Fatalf("err = %v", err)
		}
	})
}

// TestClientKnowsTheIDTheHostReturned — a client of Create, GetByID or
// BindSelf carries the ID the host returned, and its errors name it; a client
// of Upsert names its agent type until it learns the ID.
func TestClientKnowsTheIDTheHostReturned(t *testing.T) {
	id := AgentIDFromUUID(UUID{0, 0, 0, 7, 15: 4})
	known := newClient[RemoteCounterID](identity.Connection{
		TypeName: "CounterAgent", AgentID: witTypes.Some(agentIDToWit(id)), RPC: &host.WasmRpc{},
	})
	if got, err := known.AgentID(); err != nil || got != id || known.target() != id.String() {
		t.Fatalf("AgentID = %v, %v; target %q", got, err, known.target())
	}
	upserted := newClient[RemoteCounterID](identity.Connection{
		TypeName: "CounterAgent", AgentID: witTypes.None[types.AgentId](), RPC: &host.WasmRpc{},
	})
	if upserted.id.known || upserted.target() != "CounterAgent" {
		t.Fatalf("an upsert client claims an ID: %q", upserted.target())
	}
	copied := upserted
	copied.id.id, copied.id.known = id, true
	if upserted.target() != id.String() {
		t.Fatal("copies of a client do not share the learned ID")
	}
	defer func() {
		if recover() == nil {
			t.Fatal("AgentID on a zero Client did not panic")
		}
	}()
	_, _ = Client[RemoteCounterID]{}.AgentID()
}

// TestNotFoundAndConflictAreTypedRemoteCallErrors — the failures of GetByID and
// of a keyed Create reach the caller as kinds it can test with errors.As.
func TestNotFoundAndConflictAreTypedRemoteCallErrors(t *testing.T) {
	for _, c := range []struct {
		in   host.RpcError
		want RemoteCallErrorKind
	}{
		{host.MakeRpcErrorNotFound("no agent of CounterAgent has the ID"), RemoteNotFound},
		{host.MakeRpcErrorConflict("the key is current"), RemoteConflict},
	} {
		var rce *RemoteCallError
		if err := rpcErrorToGo("CounterAgent", "<create>", c.in); !errors.As(err, &rce) || rce.Kind != c.want || rce.Target != "CounterAgent" {
			t.Errorf("%v gave %v", c.want, err)
		}
	}
}
