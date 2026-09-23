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

package bridge

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/golemcloud/golem/sdks/go/core/schema"
)

// recorder is a Golem server that records what it was asked and answers with
// whatever the test set. The request bodies are the point: they are the
// contract with the real server.
type recorder struct {
	server  *httptest.Server
	paths   []string
	bodies  []map[string]any
	headers []http.Header
	respond func(path string) (int, string)
}

func newRecorder(t *testing.T) *recorder {
	t.Helper()
	r := &recorder{}
	r.server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
		raw, _ := io.ReadAll(req.Body)
		var body map[string]any
		if err := json.Unmarshal(raw, &body); err != nil {
			t.Errorf("request body is not JSON: %s", raw)
		}
		r.paths = append(r.paths, req.URL.Path)
		r.bodies = append(r.bodies, body)
		r.headers = append(r.headers, req.Header.Clone())

		status, response := 200, `{"agentId":{"componentId":"c","agentId":"a"}}`
		if r.respond != nil {
			status, response = r.respond(req.URL.Path)
		}
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(status)
		_, _ = io.WriteString(w, response)
	}))
	t.Cleanup(r.server.Close)
	return r
}

func (r *recorder) configuration() Configuration {
	return Configuration{
		Server:  Custom(r.server.URL, "token-abc"),
		AppName: "app",
		EnvName: "env",
	}
}

func (r *recorder) lastBody(t *testing.T) map[string]any {
	t.Helper()
	if len(r.bodies) == 0 {
		t.Fatal("the server was never called")
	}
	return r.bodies[len(r.bodies)-1]
}

func testAgent(t *testing.T, r *recorder, opts ...AgentOption) *Agent {
	t.Helper()
	params := schema.RecordValue{Fields: []schema.SchemaValue{
		schema.StringValue{Value: "alice"},
	}}
	opts = append([]AgentOption{WithConfiguration(r.configuration())}, opts...)
	agent, err := NewAgent("greeter", params, opts...)
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}
	return agent
}

func TestCreateAgentSendsTheConstructorArgumentsInTheWireForm(t *testing.T) {
	r := newRecorder(t)
	agent := testAgent(t, r)

	id, err := agent.Create(context.Background())
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	if id != (AgentID{ComponentID: "c", AgentID: "a"}) {
		t.Fatalf("identity read as %#v", id)
	}
	if got, ok := agent.ID(); !ok || got != id {
		t.Fatalf("the agent did not keep its identity: %#v %v", got, ok)
	}

	if r.paths[0] != "/v1/agents/create-agent" {
		t.Fatalf("called %s", r.paths[0])
	}
	if auth := r.headers[0].Get("Authorization"); auth != "Bearer token-abc" {
		t.Fatalf("authorization header was %q", auth)
	}

	body := r.lastBody(t)
	if body["appName"] != "app" || body["envName"] != "env" || body["agentTypeName"] != "greeter" {
		t.Fatalf("request named %v / %v / %v", body["appName"], body["envName"], body["agentTypeName"])
	}
	wantParams := map[string]any{
		"kind":  "record",
		"value": map[string]any{"fields": []any{map[string]any{"kind": "string", "value": "alice"}}},
	}
	if !reflect.DeepEqual(body["parameters"], wantParams) {
		t.Fatalf("parameters sent as %#v, want %#v", body["parameters"], wantParams)
	}
	// The field is not optional on the wire: an agent with no overrides must
	// still send an empty list.
	if entries, ok := body["config"].([]any); !ok || len(entries) != 0 {
		t.Fatalf("config sent as %#v, want []", body["config"])
	}
	if _, present := body["phantomId"]; present {
		t.Fatalf("phantomId should be omitted when there is none")
	}
}

// Configuration overrides are canonical JSON — the form an author writes — not
// the schema-native wire form the method parameters travel in. The server holds
// configuration as plain JSON and applies the schema itself.
func TestConfigOverridesTravelAsCanonicalJSON(t *testing.T) {
	r := newRecorder(t)
	agent := testAgent(t, r, WithConfig(
		ConfigEntry{Path: []string{"retries"}, Value: 3},
		ConfigEntry{Path: []string{"limits", "rate"}, Value: "10/s"},
	))

	if _, err := agent.Create(context.Background()); err != nil {
		t.Fatalf("Create: %v", err)
	}
	want := []any{
		map[string]any{"path": []any{"retries"}, "value": float64(3)},
		map[string]any{"path": []any{"limits", "rate"}, "value": "10/s"},
	}
	if !reflect.DeepEqual(r.lastBody(t)["config"], want) {
		t.Fatalf("config sent as %#v, want %#v", r.lastBody(t)["config"], want)
	}
}

func TestInvokeAwaitsAndReadsTheResult(t *testing.T) {
	r := newRecorder(t)
	r.respond = func(string) (int, string) {
		return 200, `{"agentId":{"componentId":"c","agentId":"a"},"idempotencyKey":"k1",
			"result":{"graph":{"root":{"kind":"string","value":{}}},
			"value":{"kind":"string","value":"hi alice"}}}`
	}
	agent := testAgent(t, r)

	result, err := agent.Invoke(context.Background(), "greet", schema.RecordValue{})
	if err != nil {
		t.Fatalf("Invoke: %v", err)
	}
	if result.IdempotencyKey != "k1" {
		t.Fatalf("idempotency key read as %q", result.IdempotencyKey)
	}
	if !reflect.DeepEqual(result.Value, schema.StringValue{Value: "hi alice"}) {
		t.Fatalf("value read as %#v", result.Value)
	}
	graph, err := result.SchemaGraph()
	if err != nil {
		t.Fatalf("SchemaGraph: %v", err)
	}
	if _, ok := graph.Root.Body.(schema.StringType); !ok {
		t.Fatalf("graph root read as %T", graph.Root.Body)
	}

	body := r.lastBody(t)
	if body["methodName"] != "greet" || body["mode"] != "await" {
		t.Fatalf("invoked %v in mode %v", body["methodName"], body["mode"])
	}
	if _, present := body["scheduleAt"]; present {
		t.Fatalf("scheduleAt should be omitted for an awaited call")
	}
	// The constructor arguments travel with every invocation, so the server can
	// resolve the agent whether or not it was created first.
	if body["parameters"] == nil {
		t.Fatalf("the constructor arguments were not sent")
	}
}

// A method that returns nothing comes back as a result object with no value,
// which is not the same as a malformed response.
func TestAUnitResultIsNoValueRatherThanAnError(t *testing.T) {
	for _, response := range []string{
		`{"agentId":{"componentId":"c","agentId":"a"},"idempotencyKey":"k"}`,
		`{"agentId":{"componentId":"c","agentId":"a"},"idempotencyKey":"k","result":null}`,
		`{"agentId":{"componentId":"c","agentId":"a"},"idempotencyKey":"k",
			"result":{"graph":{"root":{"kind":"string","value":{}}},"value":null}}`,
	} {
		r := newRecorder(t)
		r.respond = func(string) (int, string) { return 200, response }
		result, err := testAgent(t, r).Invoke(context.Background(), "ping", schema.RecordValue{})
		if err != nil {
			t.Fatalf("%s: %v", response, err)
		}
		if result.Value != nil {
			t.Fatalf("%s: value read as %#v, want none", response, result.Value)
		}
	}
}

func TestTriggerAndScheduleUseTheScheduleMode(t *testing.T) {
	r := newRecorder(t)
	r.respond = func(string) (int, string) {
		return 200, `{"agentId":{"componentId":"c","agentId":"a"},"idempotencyKey":"k2"}`
	}
	agent := testAgent(t, r)

	receipt, err := agent.Trigger(context.Background(), "notify", schema.RecordValue{})
	if err != nil {
		t.Fatalf("Trigger: %v", err)
	}
	if receipt.IdempotencyKey != "k2" {
		t.Fatalf("receipt read as %#v", receipt)
	}
	body := r.lastBody(t)
	if body["mode"] != "schedule" {
		t.Fatalf("triggered in mode %v", body["mode"])
	}
	if _, present := body["scheduleAt"]; present {
		t.Fatalf("a trigger must not name a time")
	}

	when := time.Date(2026, 3, 4, 5, 6, 7, 0, time.UTC)
	if _, err := agent.ScheduleAt(context.Background(), "notify", schema.RecordValue{}, when); err != nil {
		t.Fatalf("ScheduleAt: %v", err)
	}
	body = r.lastBody(t)
	if body["mode"] != "schedule" || body["scheduleAt"] != "2026-03-04T05:06:07Z" {
		t.Fatalf("scheduled as %v at %v", body["mode"], body["scheduleAt"])
	}
}

func TestPhantomAgentsNameTheirID(t *testing.T) {
	r := newRecorder(t)
	agent := testAgent(t, r, WithPhantomID("session-7"))
	if _, err := agent.Create(context.Background()); err != nil {
		t.Fatalf("Create: %v", err)
	}
	if got := r.lastBody(t)["phantomId"]; got != "session-7" {
		t.Fatalf("phantomId sent as %v", got)
	}
}

func TestAFailedCallSaysWhatTheServerAnswered(t *testing.T) {
	r := newRecorder(t)
	r.respond = func(string) (int, string) { return 409, `{"error":"already exists"}` }

	_, err := testAgent(t, r).Invoke(context.Background(), "greet", schema.RecordValue{})
	var bridgeErr *Error
	if !errors.As(err, &bridgeErr) {
		t.Fatalf("got %T (%v), want a *bridge.Error", err, err)
	}
	if bridgeErr.Endpoint != "invoke-agent" || bridgeErr.Status != 409 {
		t.Fatalf("error read as %#v", bridgeErr)
	}
	if !strings.Contains(bridgeErr.Error(), "already exists") {
		t.Fatalf("the error does not quote the server: %v", bridgeErr)
	}
}

func TestAnUnreachableServerIsNotAnHTTPStatus(t *testing.T) {
	// A server that has been shut down: the connection is refused, so the call
	// never reaches a server and there is no status to report.
	closed := httptest.NewServer(http.HandlerFunc(func(http.ResponseWriter, *http.Request) {}))
	closed.Close()
	configuration := Configuration{
		Server:  Custom(closed.URL, "t"),
		AppName: "app",
		EnvName: "env",
	}
	agent, err := NewAgent("greeter", schema.RecordValue{}, WithConfiguration(configuration))
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}
	_, err = agent.Create(context.Background())
	var bridgeErr *Error
	if !errors.As(err, &bridgeErr) {
		t.Fatalf("got %T (%v), want a *bridge.Error", err, err)
	}
	if bridgeErr.Status != 0 || bridgeErr.Err == nil {
		t.Fatalf("error read as %#v", bridgeErr)
	}
}

func TestACancelledContextStopsTheCall(t *testing.T) {
	r := newRecorder(t)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	_, err := testAgent(t, r).Create(ctx)
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("got %v, want a cancellation", err)
	}
}

// An agent keeps the server it was resolved against, so repointing the ambient
// configuration does not move calls already in flight to a different Golem.
func TestAnAgentKeepsTheConfigurationItWasBuiltWith(t *testing.T) {
	first := newRecorder(t)
	second := newRecorder(t)
	if err := Configure(first.configuration()); err != nil {
		t.Fatalf("Configure: %v", err)
	}
	t.Cleanup(func() {
		ambient.Lock()
		ambient.configuration = nil
		ambient.Unlock()
	})

	agent, err := NewAgent("greeter", schema.RecordValue{})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}
	if err := Configure(second.configuration()); err != nil {
		t.Fatalf("Configure: %v", err)
	}
	if _, err := agent.Create(context.Background()); err != nil {
		t.Fatalf("Create: %v", err)
	}
	if len(first.bodies) != 1 || len(second.bodies) != 0 {
		t.Fatalf("the call went to the wrong server: %d / %d", len(first.bodies), len(second.bodies))
	}
}

func TestAnUnconfiguredBridgeSaysWhatToCall(t *testing.T) {
	ambient.Lock()
	ambient.configuration = nil
	ambient.Unlock()

	_, err := NewAgent("greeter", schema.RecordValue{})
	if err == nil || !strings.Contains(err.Error(), "bridge.Configure") {
		t.Fatalf("got %v, want an error naming bridge.Configure", err)
	}
}

func TestConfigureRejectsAnIncompleteConfiguration(t *testing.T) {
	cases := map[string]Configuration{
		"server":      {AppName: "a", EnvName: "e"},
		"application": {Server: Local(), EnvName: "e"},
		"environment": {Server: Local(), AppName: "a"},
	}
	for want, configuration := range cases {
		if err := Configure(configuration); err == nil || !strings.Contains(err.Error(), want) {
			t.Fatalf("got %v, want an error mentioning %q", err, want)
		}
	}
}

func TestACustomServerURLLosesItsTrailingSlash(t *testing.T) {
	// The endpoint path is appended directly, so a trailing slash would produce
	// a double slash the router does not match.
	if got := Custom("http://example.test/", "t").URL(); got != "http://example.test" {
		t.Fatalf("URL read as %q", got)
	}
}
