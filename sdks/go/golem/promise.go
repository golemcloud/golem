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

	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Promises are Golem's durable primitive for human-in-the-loop and asynchronous
// external integration. An agent creates a promise, hands its [PromiseID] to the
// outside world, and awaits it; the invocation durably SUSPENDS — surviving
// restarts, exactly-once via oplog replay — until someone completes it with a
// payload.
//
// Completion can come from another agent ([CompletePromise]) or from off-platform
// via the worker REST API (POST /:component/workers/:agent/complete with the
// PromiseID's fields), so the completion payload is arbitrary bytes with no
// predefined meaning. [Promise] types those bytes as T via JSON, so an external
// completer (a webhook, a curl, a human) can produce them. As an escape hatch, a
// Promise[[]byte] carries the raw bytes through unchanged.

// PromiseID is the durable, serializable identity of a promise. Hand it to
// whoever will complete the promise — another agent, or an external system via
// the worker-service complete endpoint (which needs exactly these three fields).
// The exported fields make it directly JSON-marshalable.
type PromiseID struct {
	// ComponentID is the component the owning agent belongs to.
	ComponentID UUID
	// AgentID is the string agent id (agent type + constructor parameters) of the
	// agent that created the promise — the ":agent" path segment of the REST
	// complete endpoint.
	AgentID string
	// OplogIndex disambiguates promises created by the same agent — the "oplogIdx"
	// field of the REST complete endpoint.
	OplogIndex uint64
}

// String renders a compact, human-readable form (not a parse target).
func (id PromiseID) String() string {
	return fmt.Sprintf("%s/%s/%d", id.ComponentID, id.AgentID, id.OplogIndex)
}

func promiseIDFromWit(w types.PromiseId) PromiseID {
	return PromiseID{
		ComponentID: uuidFromWit(w.AgentId.ComponentId.Uuid),
		AgentID:     w.AgentId.AgentId,
		OplogIndex:  w.OplogIdx,
	}
}

func (id PromiseID) toWit() types.PromiseId {
	return types.PromiseId{
		AgentId: types.AgentId{
			ComponentId: types.ComponentId{Uuid: uuidToWit(id.ComponentID)},
			AgentId:     id.AgentID,
		},
		OplogIdx: id.OplogIndex,
	}
}

// Promise is a handle to a durable promise whose completion payload is T. Obtain
// one with [NewPromise] inside an agent, or rebuild it from a stored id with
// [PromiseByID]. Only the agent that created the promise may [Promise.Await] it.
type Promise[T any] struct {
	id PromiseID
}

// NewPromise creates a fresh promise owned by the current agent and returns a
// handle to it. Call it from inside an agent (a method or the constructor); the
// host mints the id. Persist [Promise.ID] in the agent's state (or send it out)
// if the promise will be awaited or completed in a later invocation.
func NewPromise[T any]() *Promise[T] {
	return &Promise[T]{id: promiseIDFromWit(apiHost.CreatePromise())}
}

// PromiseByID rebuilds a handle to an existing promise from a stored [PromiseID]
// — e.g. an id saved in agent state during an earlier invocation, so a later
// invocation of the same agent can [Promise.Await] it.
func PromiseByID[T any](id PromiseID) *Promise[T] {
	return &Promise[T]{id: id}
}

// ID returns the durable identity of the promise, to persist or hand to a
// completer.
func (p *Promise[T]) ID() PromiseID { return p.id }

// Await blocks until the promise is completed and returns the payload decoded as
// T (JSON, or the raw bytes when T is []byte). It durably SUSPENDS the invocation
// — the worker may be evicted and resumed, and on replay the recorded payload is
// returned from the oplog (exactly-once). Like [Future.Get] it is fail-loud: an
// infra failure traps and surfaces as an agent-error rather than returning an
// error value.
//
// Only the agent that created the promise may Await it; awaiting from another
// agent traps in the host.
func (p *Promise[T]) Await() T {
	res := apiHost.GetPromise(p.id.toWit())
	data := res.Get()
	res.Drop()
	return decodePromisePayload[T](data)
}

// WebhookURL mints an external URL that completes this promise: a POST to the URL
// completes the promise with the request body, so a later [Promise.Await] returns
// that body decoded as T (JSON, or the raw bytes when T is []byte). Hand the URL
// to an off-platform system to drive a webhook-style callback.
//
// It requires the agent type to be currently deployed behind an HTTP API (set
// [Spec].HTTP on the agent and declare the agent in the manifest's httpApi
// deployment) — it is fail-loud: the runtime traps, surfacing an agent-error, if
// the agent is not http-api-deployed, or if a different agent type created the
// promise. Typically called once per promise.
func (p *Promise[T]) WebhookURL() string {
	res := host.CreateWebhook(p.id.toWit())
	if res.IsErr() {
		panic(webhookErrorToGo(res.Err()))
	}
	return res.Ok()
}

func webhookErrorToGo(e host.WebhookError) error {
	switch e.Tag() {
	case host.WebhookErrorPermissionDenied:
		return fmt.Errorf("golem: webhook: permission denied")
	case host.WebhookErrorInternalError:
		return fmt.Errorf("golem: webhook: internal error: %s", e.InternalError())
	default:
		return fmt.Errorf("golem: webhook: error (tag %d)", e.Tag())
	}
}

// CompletePromise completes the promise identified by id, from within any agent
// (agent-to-agent completion; external completers use the REST endpoint instead).
// The value is encoded as the payload (JSON, or raw bytes when T is []byte).
//
// It returns true if this call completed the promise, false if it was already
// completed — the idempotency signal from the host. It is fail-loud only on an
// infra/encode failure.
func CompletePromise[T any](id PromiseID, value T) bool {
	return apiHost.CompletePromise(id.toWit(), encodePromisePayload(value))
}

// encodePromisePayload serializes a promise payload: raw pass-through when T is
// exactly []byte (so non-JSON external payloads round-trip), otherwise JSON.
// Pure (no host import), so it is natively testable.
func encodePromisePayload[T any](v T) []byte {
	if b, ok := any(v).([]byte); ok {
		return b
	}
	b, err := json.Marshal(v)
	if err != nil {
		panic(fmt.Errorf("golem: encoding promise payload: %w", err))
	}
	return b
}

// decodePromisePayload is the inverse of [encodePromisePayload].
func decodePromisePayload[T any](data []byte) T {
	var out T
	if _, ok := any(out).([]byte); ok {
		return any(data).(T)
	}
	if err := json.Unmarshal(data, &out); err != nil {
		panic(fmt.Errorf("golem: decoding promise payload: %w", err))
	}
	return out
}
