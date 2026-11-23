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
	"net/url"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/golemcloud/golem/sdks/go/core/schema"
)

const (
	testEnvironmentID = "11111111-1111-4111-8111-111111111111"
	testAgentID       = "00000007-0000-8000-8000-000000000004"
	testFingerprint   = "22222222-2222-4222-8222-222222222222"
	testAccountID     = "33333333-3333-4333-8333-333333333333"
	sharedEnvID       = "44444444-4444-4444-8444-444444444444"
	sharingAccountID  = "55555555-5555-4555-8555-555555555555"
	tokenPath         = "/v1/me/token"
	visibleEnvPath    = "/v1/me/visible-environments"
	greeterPath       = "/v1/runtime/envs/" + testEnvironmentID + "/agent-types/greeter"
	tokenAnswer       = `{"accountId":"` + testAccountID + `"}`
)

// visibleEnvironment is one entry of a page of visible environments.
func visibleEnvironment(id, account, app, env string) string {
	return `{"environment":{"id":"` + id + `","name":"` + env + `"},"application":{"name":"` + app + `"},"account":{"id":"` + account + `"}}`
}

// ownEnvironment is the caller's environment "env" of the application "app".
var ownEnvironment = visibleEnvironment(testEnvironmentID, testAccountID, "app", "env")

// sharedEnvironment has the same names as ownEnvironment and is shared with
// the caller by another account.
var sharedEnvironment = visibleEnvironment(sharedEnvID, sharingAccountID, "app", "env")

func testAgentIDValue(t *testing.T) AgentID {
	t.Helper()
	var id AgentID
	if err := id.UnmarshalText([]byte(testAgentID)); err != nil {
		t.Fatal(err)
	}
	return id
}

// recorder is a Golem server that records what it was asked and answers with
// whatever the test set. The request bodies are the point: they are the
// contract with the real server. The environment lookup is answered by the
// recorder itself and counted, not recorded.
type recorder struct {
	server      *httptest.Server
	methods     []string
	paths       []string
	bodies      []map[string]any
	headers     []http.Header
	tokenCalls  int
	envQueries  []string
	respond     func(path string) (int, string)
	visibleEnvs []string
}

// defaultAnswer is the answer of a server that knows the agent.
func defaultAnswer(path string) (int, string) {
	switch {
	case strings.HasSuffix(path, "/invoke"):
		return 200, `{"agentId":"` + testAgentID + `","idempotencyKey":"k"}`
	case strings.HasSuffix(path, "/agents"):
		return 201, `{"agentId":"` + testAgentID + `","agentFingerprint":"` + testFingerprint + `"}`
	case strings.HasSuffix(path, "/agents/upsert"):
		return 200, `{"agentId":"` + testAgentID + `","agentFingerprint":"` + testFingerprint + `"}`
	case strings.Contains(path, "/agents/"):
		return 200, `{"agentId":"` + testAgentID + `","agentTypeName":"greeter","fingerprint":"` + testFingerprint + `"}`
	}
	return 404, `{"error":"no route"}`
}

func newRecorder(t *testing.T) *recorder {
	t.Helper()
	r := &recorder{visibleEnvs: []string{ownEnvironment}}
	r.server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		switch req.URL.Path {
		case tokenPath:
			r.tokenCalls++
			_, _ = io.WriteString(w, tokenAnswer)
			return
		case visibleEnvPath:
			r.envQueries = append(r.envQueries, req.URL.RawQuery)
			_, _ = io.WriteString(w, `{"values":[`+strings.Join(r.visibleEnvs, ",")+`]}`)
			return
		}
		raw, _ := io.ReadAll(req.Body)
		var body map[string]any
		if len(raw) > 0 {
			if err := json.Unmarshal(raw, &body); err != nil {
				t.Errorf("request body is not JSON: %s", raw)
			}
		}
		r.methods = append(r.methods, req.Method)
		r.paths = append(r.paths, req.URL.Path)
		r.bodies = append(r.bodies, body)
		r.headers = append(r.headers, req.Header.Clone())

		respond := r.respond
		if respond == nil {
			respond = defaultAnswer
		}
		status, response := respond(req.URL.Path)
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

var aliceParams = schema.RecordValue{Fields: []schema.SchemaValue{schema.StringValue{Value: "alice"}}}

var aliceWire = map[string]any{
	"kind":  "record",
	"value": map[string]any{"fields": []any{map[string]any{"kind": "string", "value": "alice"}}},
}

func testAgent(t *testing.T, r *recorder) *Agent {
	t.Helper()
	agent, err := Upsert("greeter", aliceParams, WithConfiguration(r.configuration()))
	if err != nil {
		t.Fatalf("Upsert: %v", err)
	}
	return agent
}

func createdAgent(t *testing.T, r *recorder, opts ...CreateOption) *Agent {
	t.Helper()
	opts = append([]CreateOption{WithConfiguration(r.configuration())}, opts...)
	agent, err := Create(context.Background(), "greeter", aliceParams, opts...)
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	return agent
}

// Upsert contacts nothing; the first AgentID upserts the keyed agent of the
// constructor arguments, and later ones reuse the ID.
func TestUpsertAsksForTheIDOnceOnFirstUse(t *testing.T) {
	r := newRecorder(t)
	agent := testAgent(t, r)
	if len(r.paths) != 0 || len(r.envQueries) != 0 {
		t.Fatalf("Upsert contacted the server: %v %v", r.paths, r.envQueries)
	}
	for range 2 {
		id, err := agent.AgentID(context.Background())
		if err != nil {
			t.Fatalf("AgentID: %v", err)
		}
		if id != testAgentIDValue(t) {
			t.Fatalf("ID read as %v", id)
		}
	}
	if len(r.paths) != 1 || r.methods[0] != http.MethodPost || r.paths[0] != greeterPath+"/agents/upsert" {
		t.Fatalf("called %v %v", r.methods, r.paths)
	}
	if auth := r.headers[0].Get("Authorization"); auth != "Bearer token-abc" {
		t.Fatalf("authorization header was %q", auth)
	}
	want := map[string]any{"parameters": aliceWire}
	if !reflect.DeepEqual(r.bodies[0], want) {
		t.Fatalf("upsert sent %#v, want %#v", r.bodies[0], want)
	}
}

// The runtime API is scoped by environment ID; the configuration names the
// environment, so the bridge looks it up by name, once per configuration.
func TestTheEnvironmentIsLookedUpByNameOnce(t *testing.T) {
	r := newRecorder(t)
	agent := testAgent(t, r)
	for range 2 {
		if _, err := agent.Invoke(context.Background(), "greet", schema.RecordValue{}); err != nil {
			t.Fatalf("Invoke: %v", err)
		}
	}
	if len(r.envQueries) != 1 {
		t.Fatalf("looked the environment up %d times", len(r.envQueries))
	}
	query, err := url.ParseQuery(r.envQueries[0])
	if err != nil || query.Get("app_name") != "app" || query.Get("env_name") != "env" {
		t.Fatalf("looked up %q", r.envQueries[0])
	}
}

func TestAnUnknownEnvironmentIsNotFound(t *testing.T) {
	r := newRecorder(t)
	r.visibleEnvs = nil
	_, err := testAgent(t, r).Invoke(context.Background(), "greet", schema.RecordValue{})
	if !errors.Is(err, ErrNotFound) || !strings.Contains(err.Error(), "env") {
		t.Fatalf("got %v, want a not-found error naming the environment", err)
	}
	if len(r.paths) != 0 {
		t.Fatalf("called %v", r.paths)
	}
}

// Another account can share an environment with the same names; the caller's
// own environment is the one used, whatever the order of the page.
func TestASharedEnvironmentWithTheSameNamesIsNotUsed(t *testing.T) {
	r := newRecorder(t)
	r.visibleEnvs = []string{sharedEnvironment, ownEnvironment}
	if _, err := testAgent(t, r).Invoke(context.Background(), "greet", schema.RecordValue{}); err != nil {
		t.Fatalf("Invoke: %v", err)
	}
	if len(r.paths) != 1 || r.paths[0] != greeterPath+"/invoke" {
		t.Fatalf("called %v", r.paths)
	}
}

// A caller that sees the names only in an environment of another account has
// no such environment.
func TestOnlyASharedEnvironmentIsNotFound(t *testing.T) {
	r := newRecorder(t)
	r.visibleEnvs = []string{
		sharedEnvironment,
		visibleEnvironment(sharedEnvID, testAccountID, "app", "env-2"),
		visibleEnvironment(sharedEnvID, testAccountID, "app-2", "env"),
	}
	_, err := testAgent(t, r).Invoke(context.Background(), "greet", schema.RecordValue{})
	if !errors.Is(err, ErrNotFound) || !strings.Contains(err.Error(), "account of the token") {
		t.Fatalf("got %v, want a not-found error naming the account", err)
	}
	if len(r.paths) != 0 {
		t.Fatalf("called %v", r.paths)
	}
}

// The resolved environment is shared by every client of the same
// configuration, not held by one client.
func TestClientsWithTheSameConfigurationResolveTheEnvironmentOnce(t *testing.T) {
	r := newRecorder(t)
	for range 2 {
		if _, err := testAgent(t, r).Invoke(context.Background(), "greet", schema.RecordValue{}); err != nil {
			t.Fatalf("Invoke: %v", err)
		}
	}
	if r.tokenCalls != 1 || len(r.envQueries) != 1 {
		t.Fatalf("asked for the token %d times and the environments %d times", r.tokenCalls, len(r.envQueries))
	}
	if len(r.paths) != 2 {
		t.Fatalf("called %v", r.paths)
	}
}

func TestCreateSendsTheConstructorArgumentsInTheWireForm(t *testing.T) {
	r := newRecorder(t)
	agent := createdAgent(t, r)

	if r.methods[0] != http.MethodPost || r.paths[0] != greeterPath+"/agents" {
		t.Fatalf("called %s %s", r.methods[0], r.paths[0])
	}
	body := r.lastBody(t)
	if !reflect.DeepEqual(body["parameters"], aliceWire) {
		t.Fatalf("parameters sent as %#v, want %#v", body["parameters"], aliceWire)
	}
	// The fields default on the server, but are sent: an agent with no
	// overrides sends an empty list and an empty map.
	if entries, ok := body["config"].([]any); !ok || len(entries) != 0 {
		t.Fatalf("config sent as %#v, want []", body["config"])
	}
	if env, ok := body["env"].(map[string]any); !ok || len(env) != 0 {
		t.Fatalf("env sent as %#v, want {}", body["env"])
	}
	if body["keyed"] != false {
		t.Fatalf("keyed sent as %#v, want false", body["keyed"])
	}

	id, err := agent.AgentID(context.Background())
	if err != nil || id != testAgentIDValue(t) {
		t.Fatalf("AgentID = %v, %v", id, err)
	}
	if len(r.paths) != 1 {
		t.Fatalf("a created agent asked for its ID again: %v", r.paths)
	}
}

func TestKeyedReachesTheCreateRequest(t *testing.T) {
	r := newRecorder(t)
	createdAgent(t, r, Keyed())
	if got := r.lastBody(t)["keyed"]; got != true {
		t.Fatalf("keyed sent as %#v, want true", got)
	}
}

// Configuration overrides are canonical JSON — the form an author writes — not
// the schema-native wire form the method parameters travel in. The server holds
// configuration as plain JSON and applies the schema itself.
func TestCreateOptionsReachTheServer(t *testing.T) {
	r := newRecorder(t)
	createdAgent(t, r,
		WithConfig(
			ConfigEntry{Path: []string{"retries"}, Value: 3},
			ConfigEntry{Path: []string{"limits", "rate"}, Value: "10/s"},
		),
		WithTypedConfig(TypedConfigEntry{
			Path:  []string{"name"},
			Type:  `{"root":{"kind":"string","value":{}}}`,
			Value: func() schema.SchemaValue { return schema.StringValue{Value: "bob"} },
		}),
		WithEnv("REGION", "eu"),
		WithEnv("TIER", "gold"),
	)
	body := r.lastBody(t)
	wantConfig := []any{
		map[string]any{"path": []any{"retries"}, "value": float64(3)},
		map[string]any{"path": []any{"limits", "rate"}, "value": "10/s"},
		map[string]any{"path": []any{"name"}, "value": "bob"},
	}
	if !reflect.DeepEqual(body["config"], wantConfig) {
		t.Fatalf("config sent as %#v, want %#v", body["config"], wantConfig)
	}
	wantEnv := map[string]any{"REGION": "eu", "TIER": "gold"}
	if !reflect.DeepEqual(body["env"], wantEnv) {
		t.Fatalf("env sent as %#v, want %#v", body["env"], wantEnv)
	}
}

func TestAKeyedCreateOfACurrentKeyIsAConflict(t *testing.T) {
	r := newRecorder(t)
	r.respond = func(string) (int, string) { return 409, `{"error":"already exists"}` }
	_, err := Create(context.Background(), "greeter", aliceParams, WithConfiguration(r.configuration()), Keyed())
	if !errors.Is(err, ErrConflict) || errors.Is(err, ErrNotFound) {
		t.Fatalf("got %v, want a conflict", err)
	}
	var bridgeErr *Error
	if !errors.As(err, &bridgeErr) || bridgeErr.Endpoint != "create-agent" || bridgeErr.Status != 409 {
		t.Fatalf("error read as %#v", err)
	}
	if !strings.Contains(err.Error(), "already exists") {
		t.Fatalf("the error does not quote the server: %v", err)
	}
}

// A created or found agent is invoked by its ID with its fingerprint pinned,
// never by constructor arguments that could reach another agent.
func TestAnAgentKnownByIDIsInvokedByIDWithItsFingerprint(t *testing.T) {
	r := newRecorder(t)
	created := createdAgent(t, r)
	found, err := GetByID(context.Background(), "greeter", testAgentIDValue(t), WithConfiguration(r.configuration()))
	if err != nil {
		t.Fatalf("GetByID: %v", err)
	}
	if r.methods[1] != http.MethodGet || r.paths[1] != "/v1/runtime/envs/"+testEnvironmentID+"/agents/"+testAgentID {
		t.Fatalf("GetByID called %s %s", r.methods[1], r.paths[1])
	}
	for _, agent := range []*Agent{created, found} {
		if _, err := agent.Invoke(context.Background(), "greet", schema.RecordValue{}); err != nil {
			t.Fatalf("Invoke: %v", err)
		}
		body := r.lastBody(t)
		if r.paths[len(r.paths)-1] != greeterPath+"/invoke" {
			t.Fatalf("invoked at %s", r.paths[len(r.paths)-1])
		}
		if body["agentId"] != testAgentID || body["expectedFingerprint"] != testFingerprint {
			t.Fatalf("named the agent as %v / %v", body["agentId"], body["expectedFingerprint"])
		}
		if _, present := body["parameters"]; present {
			t.Fatalf("an agent known by ID sent constructor arguments")
		}
	}
}

func TestGetByIDOfAMissingAgentIsNotFound(t *testing.T) {
	r := newRecorder(t)
	r.respond = func(string) (int, string) { return 404, `{"error":"no such agent"}` }
	_, err := GetByID(context.Background(), "greeter", testAgentIDValue(t), WithConfiguration(r.configuration()))
	if !errors.Is(err, ErrNotFound) || errors.Is(err, ErrConflict) {
		t.Fatalf("got %v, want not found", err)
	}
}

// An agent of another type is not found as this type, so a typed client can
// never address an agent its methods do not belong to.
func TestGetByIDOfAnAgentOfAnotherTypeIsNotFound(t *testing.T) {
	r := newRecorder(t)
	r.respond = func(string) (int, string) {
		return 200, `{"agentId":"` + testAgentID + `","agentTypeName":"counter","fingerprint":"` + testFingerprint + `"}`
	}
	_, err := GetByID(context.Background(), "greeter", testAgentIDValue(t), WithConfiguration(r.configuration()))
	if !errors.Is(err, ErrNotFound) || !strings.Contains(err.Error(), "greeter") {
		t.Fatalf("got %v, want not found", err)
	}
}

func TestSessionTargets(t *testing.T) {
	for _, name := range []string{"upsert", "upsert-with-id", "create", "keyed-create", "get-by-id"} {
		t.Run(name, func(t *testing.T) {
			wantTarget := map[string]any{"kind": "id", "agentId": testAgentID, "expectedFingerprint": testFingerprint}
			if strings.HasPrefix(name, "upsert") {
				wantTarget = map[string]any{"kind": "key", "constructorParameters": aliceWire}
			}
			f := newFakeServer(t, func(c *fakeConn) {
				start := c.expect("invocationStart")
				want := map[string]any{
					"application": "app", "environment": "env", "agentType": "greeter",
					"method": "upload", "target": wantTarget,
				}
				if !reflect.DeepEqual(start.m["selector"], want) {
					t.Errorf("selector = %#v, want %#v", start.m["selector"], want)
				}
				refs := provisionalRefs(t, start)
				accept(c, start, "session", inputMapping(1, refs[0], "in", 0, false))
				if got := consumeInput(c, 1); !reflect.DeepEqual(got, []any{float64(1), float64(2)}) {
					t.Errorf("input = %v", got)
				}
				c.send("invocationResult", map[string]any{"mappings": []any{}, "result": map[string]any{"kind": "none"}})
				finished(c)
				c.waitClose()
			})
			r := newRecorder(t)
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
				if strings.HasSuffix(req.URL.Path, sessionEndpoint) {
					f.srv.Config.Handler.ServeHTTP(w, req)
				} else {
					r.server.Config.Handler.ServeHTTP(w, req)
				}
			}))
			t.Cleanup(server.Close)
			config := Configuration{Server: Custom(server.URL, "token"), AppName: "app", EnvName: "env"}
			environmentIDs.Store(environmentKeyOf(config), testEnvironmentID)
			t.Cleanup(func() { environmentIDs.Delete(environmentKeyOf(config)) })
			ctx := ctxT(t)
			var agent *Agent
			var err error
			switch name {
			case "upsert", "upsert-with-id":
				agent, err = Upsert("greeter", aliceParams, WithConfiguration(config))
			case "create":
				agent, err = Create(ctx, "greeter", aliceParams, WithConfiguration(config))
			case "keyed-create":
				agent, err = Create(ctx, "greeter", aliceParams, WithConfiguration(config), Keyed())
			case "get-by-id":
				agent, err = GetByID(ctx, "greeter", testAgentIDValue(t), WithConfiguration(config))
			}
			if err != nil {
				t.Fatal(err)
			}
			if name == "upsert-with-id" {
				if id, err := agent.AgentID(ctx); err != nil || id != testAgentIDValue(t) {
					t.Fatalf("AgentID = %v, %v", id, err)
				}
			}
			if err := InvokeStreaming(ctx, agent, "upload", streamParams(EncodeStream(EncodeU32)(StreamOf[uint32](1, 2)))); err != nil {
				t.Fatal(err)
			}
		})
	}
}

func TestInvokeAwaitsAndReadsTheResult(t *testing.T) {
	r := newRecorder(t)
	r.respond = func(string) (int, string) {
		return 200, `{"agentId":"` + testAgentID + `","idempotencyKey":"k1",
			"result":{"graph":{"root":{"kind":"string","value":{}}},
			"value":{"kind":"string","value":"hi alice"}}}`
	}
	agent := testAgent(t, r)

	result, err := agent.Invoke(context.Background(), "greet", schema.RecordValue{})
	if err != nil {
		t.Fatalf("Invoke: %v", err)
	}
	if result.IdempotencyKey != "k1" || result.AgentID != testAgentIDValue(t) {
		t.Fatalf("result read as %v / %q", result.AgentID, result.IdempotencyKey)
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

	if r.paths[0] != greeterPath+"/invoke" {
		t.Fatalf("invoked at %s", r.paths[0])
	}
	body := r.lastBody(t)
	if body["method"] != "greet" || body["mode"] != "await" {
		t.Fatalf("invoked %v in mode %v", body["method"], body["mode"])
	}
	if _, present := body["scheduleAt"]; present {
		t.Fatalf("scheduleAt should be omitted for an awaited call")
	}
	// An upsert agent is named by its constructor arguments, so the server
	// reaches the keyed agent whether or not it was created first.
	if !reflect.DeepEqual(body["parameters"], aliceWire) {
		t.Fatalf("parameters sent as %#v", body["parameters"])
	}
	for _, field := range []string{"agentId", "expectedFingerprint"} {
		if _, present := body[field]; present {
			t.Fatalf("an upsert agent sent %s", field)
		}
	}
}

// A method that returns nothing comes back as a result object with no value,
// which is not the same as a malformed response.
func TestAUnitResultIsNoValueRatherThanAnError(t *testing.T) {
	for _, response := range []string{
		`{"agentId":"` + testAgentID + `","idempotencyKey":"k"}`,
		`{"agentId":"` + testAgentID + `","idempotencyKey":"k","result":null}`,
		`{"agentId":"` + testAgentID + `","idempotencyKey":"k",
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
		return 200, `{"agentId":"` + testAgentID + `","idempotencyKey":"k2"}`
	}
	agent := testAgent(t, r)

	receipt, err := agent.Trigger(context.Background(), "notify", schema.RecordValue{})
	if err != nil {
		t.Fatalf("Trigger: %v", err)
	}
	if receipt.IdempotencyKey != "k2" || receipt.AgentID != testAgentIDValue(t) {
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

func TestAFailedCallSaysWhatTheServerAnswered(t *testing.T) {
	r := newRecorder(t)
	r.respond = func(string) (int, string) { return 500, `{"error":"executor unavailable"}` }

	_, err := testAgent(t, r).Invoke(context.Background(), "greet", schema.RecordValue{})
	var bridgeErr *Error
	if !errors.As(err, &bridgeErr) {
		t.Fatalf("got %T (%v), want a *bridge.Error", err, err)
	}
	if bridgeErr.Endpoint != "invoke-agent" || bridgeErr.Status != 500 {
		t.Fatalf("error read as %#v", bridgeErr)
	}
	if errors.Is(err, ErrNotFound) || errors.Is(err, ErrConflict) {
		t.Fatalf("a server failure matched a typed error: %v", err)
	}
	if !strings.Contains(bridgeErr.Error(), "executor unavailable") {
		t.Fatalf("the error does not quote the server: %v", bridgeErr)
	}
}

// An invocation of an agent that is gone, or whose pinned fingerprint is no
// longer current, is a not-found error.
func TestInvokingAMissingAgentIsNotFound(t *testing.T) {
	r := newRecorder(t)
	agent := createdAgent(t, r)
	r.respond = func(string) (int, string) { return 404, `{"error":"no such agent"}` }
	if _, err := agent.Invoke(context.Background(), "greet", schema.RecordValue{}); !errors.Is(err, ErrNotFound) {
		t.Fatalf("got %v, want not found", err)
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
	_, err := Create(context.Background(), "greeter", schema.RecordValue{}, WithConfiguration(configuration))
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
	_, err := testAgent(t, r).AgentID(ctx)
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

	agent, err := Upsert("greeter", schema.RecordValue{})
	if err != nil {
		t.Fatalf("Upsert: %v", err)
	}
	if err := Configure(second.configuration()); err != nil {
		t.Fatalf("Configure: %v", err)
	}
	if _, err := agent.AgentID(context.Background()); err != nil {
		t.Fatalf("AgentID: %v", err)
	}
	if len(first.bodies) != 1 || len(second.bodies) != 0 || len(second.envQueries) != 0 {
		t.Fatalf("the call went to the wrong server: %d / %d", len(first.bodies), len(second.bodies))
	}
}

func TestAnUnconfiguredBridgeSaysWhatToCall(t *testing.T) {
	ambient.Lock()
	ambient.configuration = nil
	ambient.Unlock()

	_, err := Upsert("greeter", schema.RecordValue{})
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

// A 2xx answer the client cannot use says why, not only which status it was.
func TestAnUnreadableAnswerSaysWhy(t *testing.T) {
	for _, response := range []string{
		`not json`,
		// Duplicate members are rejected, as on the session path.
		`{"agentId":"` + testAgentID + `","agentId":"` + testAgentID + `"}`,
		// An agent ID is a UUID.
		`{"agentId":"Greeter(\"alice\")","idempotencyKey":"k"}`,
	} {
		r := newRecorder(t)
		r.respond = func(string) (int, string) { return 200, response }
		_, err := testAgent(t, r).Invoke(context.Background(), "greet", schema.RecordValue{})
		var bridgeErr *Error
		if !errors.As(err, &bridgeErr) || bridgeErr.Status != 200 || bridgeErr.Err == nil {
			t.Fatalf("%s: got %v", response, err)
		}
		if !strings.Contains(err.Error(), "could not read the response") {
			t.Fatalf("%s: the error does not say why: %v", response, err)
		}
	}
}

// A result that does not fit the graph it arrived with is rejected, as on the
// session path.
func TestAResultMustFitItsGraph(t *testing.T) {
	r := newRecorder(t)
	r.respond = func(string) (int, string) {
		return 200, `{"agentId":"` + testAgentID + `","idempotencyKey":"k1",
			"result":{"graph":{"root":{"kind":"u8","value":{}}},
			"value":{"kind":"string","value":"not a u8"}}}`
	}
	if _, err := testAgent(t, r).Invoke(context.Background(), "greet", schema.RecordValue{}); err == nil {
		t.Fatal("a result that does not fit its graph was accepted")
	}
}

// One client may be used from several goroutines; run with -race.
func TestConcurrentCallsOnOneAgent(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		switch req.URL.Path {
		case tokenPath:
			_, _ = io.WriteString(w, tokenAnswer)
			return
		case visibleEnvPath:
			_, _ = io.WriteString(w, `{"values":[`+ownEnvironment+`]}`)
			return
		}
		_, response := defaultAnswer(req.URL.Path)
		_, _ = io.WriteString(w, response)
	}))
	t.Cleanup(server.Close)
	agent, err := Upsert("greeter", schema.RecordValue{}, WithConfiguration(Configuration{
		Server: Custom(server.URL, "token"), AppName: "app", EnvName: "env",
	}))
	if err != nil {
		t.Fatal(err)
	}
	errs := make(chan error, 16)
	for range 8 {
		go func() {
			_, err := agent.Invoke(context.Background(), "ping", schema.RecordValue{})
			errs <- err
		}()
		go func() {
			_, err := agent.AgentID(context.Background())
			errs <- err
		}()
	}
	for range 16 {
		if err := <-errs; err != nil {
			t.Fatal(err)
		}
	}
	if id, err := agent.AgentID(context.Background()); err != nil || id != testAgentIDValue(t) {
		t.Fatalf("ID read as %v %v", id, err)
	}
}

// The WebSocket handshake uses the configured client's transport even when
// the client has a timeout, which the websocket library does not accept.
func TestTheDialClientKeepsTheTransport(t *testing.T) {
	transport := &http.Transport{}
	client := &http.Client{Timeout: time.Minute, Transport: transport}
	dial := dialClient(client)
	if dial.Timeout != 0 || dial.Transport != transport {
		t.Fatalf("dial client = %+v", dial)
	}
	if client.Timeout != time.Minute {
		t.Fatal("the configured client was changed")
	}
	plain := &http.Client{}
	if dialClient(plain) != plain {
		t.Fatal("a client without a timeout was copied")
	}
}

// Closing a writer wakes a Write that waits for room in a full buffer.
func TestClosingAWriterWakesABlockedWrite(t *testing.T) {
	w, _ := NewAgentStream[uint32]()
	for i := range streamBuffer {
		if err := w.Write(uint32(i)); err != nil {
			t.Fatal(err)
		}
	}
	done := make(chan error, 1)
	go func() { done <- w.Write(streamBuffer) }()
	time.Sleep(50 * time.Millisecond)
	_ = w.Close()
	select {
	case err := <-done:
		if err == nil {
			t.Fatal("a Write after Close succeeded")
		}
	case <-time.After(5 * time.Second):
		t.Fatal("Close did not wake the blocked Write")
	}
}
