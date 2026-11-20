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
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"sync"

	"github.com/golemcloud/golem/sdks/go/core/schema"
)

// maxErrorBody bounds how much of a failed response is quoted back. A server
// that answers an agent call with a page of HTML should not put that page in a
// Go error.
const maxErrorBody = 4096

// environmentKey names one resolution of an environment: the same names can
// resolve to different environments on another server or for another token.
type environmentKey struct {
	url, token, appName, envName string
}

func environmentKeyOf(c Configuration) environmentKey {
	return environmentKey{url: c.Server.URL(), token: c.Server.Token(), appName: c.AppName, envName: c.EnvName}
}

// environmentIDs caches resolved environment IDs by environmentKey for every
// client in the process.
var environmentIDs sync.Map

// environmentID resolves the configuration's application and environment
// names to the ID of the environment the runtime API is scoped by: the one in
// the account of the token. Environments that other accounts share with that
// account are visible too and may have the same names; they are never used.
func environmentID(ctx context.Context, a *Agent) (string, error) {
	c := *a.configuration
	key := environmentKeyOf(c)
	if id, ok := environmentIDs.Load(key); ok {
		return id.(string), nil
	}
	var token currentToken
	if err := send(ctx, c, http.MethodGet, "token", "/v1/me/token", nil, &token); err != nil {
		return "", err
	}
	query := url.Values{"app_name": {c.AppName}, "env_name": {c.EnvName}}
	var page visibleEnvironments
	if err := send(ctx, c, http.MethodGet, "visible-environments", "/v1/me/visible-environments?"+query.Encode(), nil, &page); err != nil {
		return "", err
	}
	for _, candidate := range page.Values {
		if candidate.Account.ID == token.AccountID &&
			candidate.Application.Name == c.AppName &&
			candidate.Environment.Name == c.EnvName {
			id := candidate.Environment.ID
			environmentIDs.Store(key, id)
			return id, nil
		}
	}
	return "", &Error{Endpoint: "visible-environments", Err: fmt.Errorf("%w: environment %s of application %s does not exist in the account of the token", ErrNotFound, c.EnvName, c.AppName)}
}

// agentTypePath is the runtime path of the agent's type under its environment.
func agentTypePath(ctx context.Context, a *Agent) (string, error) {
	env, err := environmentID(ctx, a)
	if err != nil {
		return "", err
	}
	return "/v1/runtime/envs/" + url.PathEscape(env) + "/agent-types/" + url.PathEscape(a.typeName), nil
}

func createAgent(ctx context.Context, a *Agent, keyed bool, config []ConfigEntry, env map[string]string) (agentReference, error) {
	parameters, err := schema.MarshalWireValue(a.parameters)
	if err != nil {
		return agentReference{}, fmt.Errorf("golem: %s constructor arguments: %w", a.typeName, err)
	}
	base, err := agentTypePath(ctx, a)
	if err != nil {
		return agentReference{}, err
	}
	if env == nil {
		env = map[string]string{}
	}
	request := createAgentRequest{
		Parameters: parameters,
		Config:     configEntriesToDTO(config),
		Env:        env,
		Keyed:      keyed,
	}
	var response agentReference
	if err := send(ctx, *a.configuration, http.MethodPost, "create-agent", base+"/agents", request, &response); err != nil {
		return agentReference{}, err
	}
	return response, nil
}

func upsertAgent(ctx context.Context, a *Agent) (agentReference, error) {
	if a.parameters == nil {
		return agentReference{}, fmt.Errorf("golem: %s: the agent has no constructor arguments to upsert", a.typeName)
	}
	parameters, err := schema.MarshalWireValue(a.parameters)
	if err != nil {
		return agentReference{}, fmt.Errorf("golem: %s constructor arguments: %w", a.typeName, err)
	}
	base, err := agentTypePath(ctx, a)
	if err != nil {
		return agentReference{}, err
	}
	var response agentReference
	if err := send(ctx, *a.configuration, http.MethodPost, "upsert-agent", base+"/agents/upsert", agentKeyRequest{Parameters: parameters}, &response); err != nil {
		return agentReference{}, err
	}
	return response, nil
}

func getAgent(ctx context.Context, a *Agent, id AgentID) (agentMetadata, error) {
	env, err := environmentID(ctx, a)
	if err != nil {
		return agentMetadata{}, err
	}
	path := "/v1/runtime/envs/" + url.PathEscape(env) + "/agents/" + id.String()
	var response agentMetadata
	if err := send(ctx, *a.configuration, http.MethodGet, "get-agent", path, nil, &response); err != nil {
		return agentMetadata{}, err
	}
	return response, nil
}

func invokeAgent(
	ctx context.Context,
	a *Agent,
	method string,
	parameters schema.SchemaValue,
	mode InvocationMode,
	scheduleAt *string,
) (Result, error) {
	methodArgs, err := schema.MarshalWireValue(parameters)
	if err != nil {
		return Result{}, fmt.Errorf("golem: %s.%s arguments: %w", a.typeName, method, err)
	}
	request := invokeAgentRequest{
		Method:           method,
		MethodParameters: methodArgs,
		Mode:             mode,
		ScheduleAt:       scheduleAt,
	}
	if a.byID {
		request.AgentID = a.id.Load()
		request.ExpectedFingerprint = a.fingerprint
	} else {
		constructorArgs, err := schema.MarshalWireValue(a.parameters)
		if err != nil {
			return Result{}, fmt.Errorf("golem: %s constructor arguments: %w", a.typeName, err)
		}
		request.Parameters = constructorArgs
	}
	base, err := agentTypePath(ctx, a)
	if err != nil {
		return Result{}, err
	}
	var response invokeAgentResponse
	if err := send(ctx, *a.configuration, http.MethodPost, "invoke-agent", base+"/invoke", request, &response); err != nil {
		return Result{}, err
	}

	result := Result{AgentID: response.AgentID, IdempotencyKey: response.IdempotencyKey}
	if response.Result == nil {
		return result, nil
	}
	result.Graph = response.Result.Graph
	// A present result object without a value still means "no value": that is
	// how the server encodes a method that returns unit.
	if len(response.Result.Value) == 0 || string(response.Result.Value) == "null" {
		return result, nil
	}
	value, err := schema.UnmarshalWireValue(response.Result.Value)
	if err != nil {
		return Result{}, fmt.Errorf("golem: %s.%s result: %w", a.typeName, method, err)
	}
	// As on the session path, the value must fit the graph it arrived with.
	graph, err := schema.UnmarshalWireGraph(response.Result.Graph)
	if err != nil {
		return Result{}, fmt.Errorf("golem: %s.%s result graph: %w", a.typeName, method, err)
	}
	if err := schema.NewRef(graph).Validate(value); err != nil {
		return Result{}, fmt.Errorf("golem: %s.%s result: %w", a.typeName, method, err)
	}
	result.Value = value
	return result, nil
}

// send makes one REST call. request is nil for a call without a body.
func send(
	ctx context.Context,
	configuration Configuration,
	httpMethod string,
	endpoint string,
	path string,
	request any,
	response any,
) error {
	var body io.Reader
	if request != nil {
		encoded, err := json.Marshal(request)
		if err != nil {
			return &Error{Endpoint: endpoint, Err: err}
		}
		body = bytes.NewReader(encoded)
	}
	httpRequest, err := http.NewRequestWithContext(ctx, httpMethod, configuration.Server.URL()+path, body)
	if err != nil {
		return &Error{Endpoint: endpoint, Err: err}
	}
	if request != nil {
		httpRequest.Header.Set("Content-Type", "application/json")
	}
	httpRequest.Header.Set("Authorization", "Bearer "+configuration.Server.Token())

	httpResponse, err := configuration.client().Do(httpRequest)
	if err != nil {
		return &Error{Endpoint: endpoint, Err: err}
	}
	defer func() { _ = httpResponse.Body.Close() }()

	if httpResponse.StatusCode < 200 || httpResponse.StatusCode >= 300 {
		text, _ := io.ReadAll(io.LimitReader(httpResponse.Body, maxErrorBody))
		return &Error{
			Endpoint: endpoint,
			Status:   httpResponse.StatusCode,
			Body:     string(text),
		}
	}
	// An answer that does not arrive in full did not arrive, so it has no status.
	data, err := io.ReadAll(httpResponse.Body)
	if err != nil {
		return &Error{Endpoint: endpoint, Err: fmt.Errorf("could not read the response: %w", err)}
	}
	// The response is held to the same strict JSON as a session message.
	err = schema.CheckWireJSON(data)
	if err == nil {
		err = json.Unmarshal(data, response)
	}
	if err != nil {
		return &Error{
			Endpoint: endpoint,
			Status:   httpResponse.StatusCode,
			Err:      fmt.Errorf("could not read the response: %w", err),
		}
	}
	return nil
}
