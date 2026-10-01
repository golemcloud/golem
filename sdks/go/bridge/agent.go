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
	"crypto/rand"
	"fmt"
	"time"

	"github.com/golemcloud/golem/sdks/go/core/schema"
)

// Agent is one addressable agent: its type, the constructor arguments that
// identify it, and the configuration it was resolved with. A generated client
// wraps one of these and adds a typed method per agent method.
//
// The configuration is captured when the agent is built, so an agent keeps
// talking to the server it was resolved against even if the ambient
// configuration is later repointed.
type Agent struct {
	// configuration is nil until NewAgent has settled on one, which is how an
	// explicit WithConfiguration is told apart from the ambient fallback.
	configuration *Configuration
	typeName      string
	parameters    schema.SchemaValue
	phantomID     *string
	config        []ConfigEntry
	id            *AgentID
}

// AgentOption adjusts how an agent is addressed.
type AgentOption func(*Agent)

// WithConfiguration resolves the agent against a specific configuration rather
// than the ambient one.
func WithConfiguration(c Configuration) AgentOption {
	return func(a *Agent) { a.configuration = &c }
}

// WithNewPhantomID addresses a fresh phantom instance, under a random id.
// Every client made with it reaches a different instance.
func WithNewPhantomID() AgentOption {
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		panic(fmt.Sprintf("golem: no randomness for a phantom id: %v", err))
	}
	b[6] = b[6]&0x0f | 0x40
	b[8] = b[8]&0x3f | 0x80
	return WithPhantomID(fmt.Sprintf("%x-%x-%x-%x-%x", b[0:4], b[4:6], b[6:8], b[8:10], b[10:16]))
}

// WithPhantomID addresses a phantom instance: one that exists only for the
// calls made to it, rather than being created and kept.
func WithPhantomID(id string) AgentOption {
	return func(a *Agent) { a.phantomID = &id }
}

// WithConfig overrides agent configuration values. Each value is canonical
// JSON, not a schema.SchemaValue.
func WithConfig(entries ...ConfigEntry) AgentOption {
	return func(a *Agent) { a.config = append(a.config, entries...) }
}

// NewAgent addresses an agent without contacting the server. parameters is the
// constructor's arguments as a schema-native record.
//
// Nothing is sent until the first call, so an unreachable server is reported by
// Create or Invoke rather than here.
func NewAgent(typeName string, parameters schema.SchemaValue, opts ...AgentOption) (*Agent, error) {
	a := &Agent{typeName: typeName, parameters: parameters}
	for _, opt := range opts {
		opt(a)
	}
	if a.configuration == nil {
		configuration, err := Current()
		if err != nil {
			return nil, err
		}
		a.configuration = &configuration
	}
	if err := a.configuration.validate(); err != nil {
		return nil, err
	}
	return a, nil
}

// TypeName is the agent type this agent is an instance of.
func (a *Agent) TypeName() string { return a.typeName }

// ID is the agent's identity once the server has assigned one, which happens on
// Create or on the first awaited invocation.
func (a *Agent) ID() (AgentID, bool) {
	if a.id == nil {
		return AgentID{}, false
	}
	return *a.id, true
}

// Create creates the agent, or returns the identity of the existing one.
// Invoking does this implicitly, so an explicit call is only needed to learn
// the identity up front or to fail early on a bad constructor argument.
func (a *Agent) Create(ctx context.Context) (AgentID, error) {
	id, err := createAgent(ctx, a)
	if err != nil {
		return AgentID{}, err
	}
	a.id = &id
	return id, nil
}

// Invoke runs a method and waits for its result.
func (a *Agent) Invoke(
	ctx context.Context, method string, parameters schema.SchemaValue,
) (Result, error) {
	return invokeAgent(ctx, a, method, parameters, ModeAwait, nil)
}

// Trigger enqueues a method without waiting for it. The returned receipt names
// the invocation; there is no result, because the method has not run.
func (a *Agent) Trigger(
	ctx context.Context, method string, parameters schema.SchemaValue,
) (Receipt, error) {
	result, err := invokeAgent(ctx, a, method, parameters, ModeSchedule, nil)
	if err != nil {
		return Receipt{}, err
	}
	return Receipt{AgentID: result.AgentID, IdempotencyKey: result.IdempotencyKey}, nil
}

// ScheduleAt enqueues a method to run no earlier than the given time.
func (a *Agent) ScheduleAt(
	ctx context.Context, method string, parameters schema.SchemaValue, when time.Time,
) (Receipt, error) {
	at := when.UTC().Format(time.RFC3339Nano)
	result, err := invokeAgent(ctx, a, method, parameters, ModeSchedule, &at)
	if err != nil {
		return Receipt{}, err
	}
	return Receipt{AgentID: result.AgentID, IdempotencyKey: result.IdempotencyKey}, nil
}
