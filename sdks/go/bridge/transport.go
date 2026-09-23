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

	"github.com/golemcloud/golem/sdks/go/core/schema"
)

// maxErrorBody bounds how much of a failed response is quoted back. A server
// that answers an agent call with a page of HTML should not put that page in a
// Go error.
const maxErrorBody = 4096

func createAgent(ctx context.Context, a *Agent) (AgentID, error) {
	parameters, err := schema.MarshalWireValue(a.parameters)
	if err != nil {
		return AgentID{}, fmt.Errorf("golem: %s constructor arguments: %w", a.typeName, err)
	}
	request := createAgentRequest{
		AppName:       a.configuration.AppName,
		EnvName:       a.configuration.EnvName,
		AgentTypeName: a.typeName,
		Parameters:    parameters,
		PhantomID:     a.phantomID,
		Config:        configEntriesToDTO(a.config),
	}
	var response createAgentResponse
	if err := post(ctx, *a.configuration, "create-agent", request, nil, &response); err != nil {
		return AgentID{}, err
	}
	return response.AgentID, nil
}

func invokeAgent(
	ctx context.Context,
	a *Agent,
	method string,
	parameters schema.SchemaValue,
	mode InvocationMode,
	scheduleAt *string,
) (Result, error) {
	constructorArgs, err := schema.MarshalWireValue(a.parameters)
	if err != nil {
		return Result{}, fmt.Errorf("golem: %s constructor arguments: %w", a.typeName, err)
	}
	methodArgs, err := schema.MarshalWireValue(parameters)
	if err != nil {
		return Result{}, fmt.Errorf("golem: %s.%s arguments: %w", a.typeName, method, err)
	}
	request := invokeAgentRequest{
		AppName:          a.configuration.AppName,
		EnvName:          a.configuration.EnvName,
		AgentTypeName:    a.typeName,
		Parameters:       constructorArgs,
		PhantomID:        a.phantomID,
		Config:           configEntriesToDTO(a.config),
		MethodName:       method,
		MethodParameters: methodArgs,
		Mode:             mode,
		ScheduleAt:       scheduleAt,
	}
	var response invokeAgentResponse
	if err := post(ctx, *a.configuration, "invoke-agent", request, nil, &response); err != nil {
		return Result{}, err
	}
	// The identity is learned from the first call, so a phantom or ephemeral
	// agent that was never created explicitly still reports one afterwards.
	a.id = &response.AgentID

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
	result.Value = value
	return result, nil
}

func post(
	ctx context.Context,
	configuration Configuration,
	endpoint string,
	request any,
	idempotencyKey *string,
	response any,
) error {
	body, err := json.Marshal(request)
	if err != nil {
		return &Error{Endpoint: endpoint, Err: err}
	}
	url := configuration.Server.URL() + "/v1/agents/" + endpoint
	httpRequest, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		return &Error{Endpoint: endpoint, Err: err}
	}
	httpRequest.Header.Set("Content-Type", "application/json")
	httpRequest.Header.Set("Authorization", "Bearer "+configuration.Server.Token())
	if idempotencyKey != nil {
		httpRequest.Header.Set("Idempotency-Key", *idempotencyKey)
	}

	httpResponse, err := configuration.client().Do(httpRequest)
	if err != nil {
		return &Error{Endpoint: endpoint, Err: err}
	}
	defer httpResponse.Body.Close()

	if httpResponse.StatusCode < 200 || httpResponse.StatusCode >= 300 {
		text, _ := io.ReadAll(io.LimitReader(httpResponse.Body, maxErrorBody))
		return &Error{
			Endpoint: endpoint,
			Status:   httpResponse.StatusCode,
			Body:     string(text),
		}
	}
	if err := json.NewDecoder(httpResponse.Body).Decode(response); err != nil {
		return &Error{
			Endpoint: endpoint,
			Status:   httpResponse.StatusCode,
			Err:      fmt.Errorf("could not read the response: %w", err),
		}
	}
	return nil
}
