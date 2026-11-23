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
	"fmt"
	"sync/atomic"
	"time"

	"github.com/golemcloud/golem/sdks/go/core/schema"
)

// Agent is one addressable agent of a type, and the configuration it was
// resolved with. A generated client wraps one of these and adds a typed method
// per agent method.
//
// An agent is addressed in one of two ways. A client of [Upsert] addresses the
// keyed agent of its constructor arguments, which the server creates when it is
// missing. A client of [Create] or [GetByID] addresses one agent by its ID, and
// pins the agent's fingerprint, so a deleted and re-created agent with the same
// ID is not reached by mistake.
//
// The configuration is captured when the agent is built, so an agent keeps
// talking to the server it was resolved against even if the ambient
// configuration is later repointed.
type Agent struct {
	// configuration is nil until an option or the ambient configuration has
	// settled it, which is how an explicit WithConfiguration is told apart
	// from the ambient fallback.
	configuration *Configuration
	typeName      string
	// parameters are the constructor arguments, or nil for an agent addressed
	// only by its ID.
	parameters schema.SchemaValue
	// byID makes invocations name the agent by id rather than by parameters.
	byID        bool
	fingerprint string
	// id is learned from the server and may be set by concurrent calls.
	id atomic.Pointer[AgentID]
}

// AgentOption adjusts how an agent is reached.
type AgentOption func(*Agent)

// WithConfiguration resolves the agent against a specific configuration rather
// than the ambient one.
func WithConfiguration(c Configuration) AgentOption {
	return func(a *Agent) { a.configuration = &c }
}

// CreateOption adjusts the agent that [Create] makes. An [AgentOption] is a
// CreateOption too.
type CreateOption interface{ applyCreate(*createSettings) }

func (o AgentOption) applyCreate(s *createSettings) { o(s.agent) }

type createOption func(*createSettings)

func (o createOption) applyCreate(s *createSettings) { o(s) }

type createSettings struct {
	agent       *Agent
	keyed       bool
	config      []ConfigEntry
	typedConfig []TypedConfigEntry
	env         map[string]string
}

// Keyed makes Create create the keyed agent of the constructor arguments: the
// agent that Upsert with the same arguments reaches. The create fails with an
// error that matches [ErrConflict] when that agent already exists. Without
// Keyed, Create makes a new agent with a new ID and no key.
func Keyed() CreateOption { return createOption(func(s *createSettings) { s.keyed = true }) }

// WithConfig overrides the new agent's configuration values. Each value is
// canonical JSON, not a schema.SchemaValue.
func WithConfig(entries ...ConfigEntry) CreateOption {
	return createOption(func(s *createSettings) { s.config = append(s.config, entries...) })
}

// TypedConfigEntry overrides one configuration value given as a schema value,
// with the schema graph of its type in the schema-native wire form. Generated
// clients build these from typed configuration structs. Value encodes the
// value when the agent is created, through [Encode], so a value its schema
// does not accept is reported by Create.
type TypedConfigEntry struct {
	Path  []string
	Type  string
	Value func() schema.SchemaValue
}

// WithTypedConfig overrides the new agent's configuration values given as
// schema values; each is rendered as canonical JSON against its type.
func WithTypedConfig(entries ...TypedConfigEntry) CreateOption {
	return createOption(func(s *createSettings) { s.typedConfig = append(s.typedConfig, entries...) })
}

// WithEnv adds the environment variable name to the new agent's environment,
// next to the ones its manifest defines.
func WithEnv(name, value string) CreateOption {
	return createOption(func(s *createSettings) {
		if s.env == nil {
			s.env = map[string]string{}
		}
		s.env[name] = value
	})
}

// settle applies the agent options and falls back to the ambient
// configuration.
func (a *Agent) settle() error {
	if a.configuration == nil {
		configuration, err := Current()
		if err != nil {
			return err
		}
		a.configuration = &configuration
	}
	return a.configuration.validate()
}

// Upsert addresses the keyed agent of the constructor arguments without
// contacting the server. parameters is the constructor's arguments as a
// schema-native record. The server creates the agent on the first call when it
// is missing; for an ephemeral agent, each call runs on a new one-shot
// instance.
//
// Nothing is sent until the first call, so an unreachable server is reported by
// AgentID or Invoke rather than here.
func Upsert(typeName string, parameters schema.SchemaValue, opts ...AgentOption) (*Agent, error) {
	a := &Agent{typeName: typeName, parameters: parameters}
	for _, opt := range opts {
		opt(a)
	}
	if err := a.settle(); err != nil {
		return nil, err
	}
	return a, nil
}

// Create creates a durable agent with the constructor arguments, and returns
// it once its constructor has finished. Without [Keyed] the new agent has a new
// ID and no key. Its options also set configuration overrides ([WithConfig],
// [WithTypedConfig]) and environment variables ([WithEnv]).
func Create(ctx context.Context, typeName string, parameters schema.SchemaValue, opts ...CreateOption) (*Agent, error) {
	a := &Agent{typeName: typeName, parameters: parameters, byID: true}
	settings := createSettings{agent: a}
	for _, opt := range opts {
		opt.applyCreate(&settings)
	}
	if err := a.settle(); err != nil {
		return nil, err
	}
	config := settings.config
	for _, entry := range settings.typedConfig {
		sv, err := Encode(entry.Value)
		if err != nil {
			return nil, fmt.Errorf("golem: %s config %v: %w", typeName, entry.Path, err)
		}
		graph, err := schema.UnmarshalWireGraph([]byte(entry.Type))
		if err != nil {
			return nil, fmt.Errorf("golem: %s config %v: %w", typeName, entry.Path, err)
		}
		value, err := schema.NewRef(graph).UnpackJSON(sv)
		if err != nil {
			return nil, fmt.Errorf("golem: %s config %v: %w", typeName, entry.Path, err)
		}
		config = append(config, ConfigEntry{Path: entry.Path, Value: value})
	}
	ref, err := createAgent(ctx, a, settings.keyed, config, settings.env)
	if err != nil {
		return nil, err
	}
	a.parameters = nil
	a.fingerprint = ref.AgentFingerprint
	a.id.Store(&ref.AgentID)
	return a, nil
}

// GetByID addresses the existing agent of the type with the ID. It never
// creates the agent: an unknown ID, or the ID of an agent of another type, is
// an error that matches [ErrNotFound].
func GetByID(ctx context.Context, typeName string, id AgentID, opts ...AgentOption) (*Agent, error) {
	a := &Agent{typeName: typeName, byID: true}
	for _, opt := range opts {
		opt(a)
	}
	if err := a.settle(); err != nil {
		return nil, err
	}
	metadata, err := getAgent(ctx, a, id)
	if err != nil {
		return nil, err
	}
	if metadata.AgentTypeName == nil || *metadata.AgentTypeName != typeName {
		return nil, &Error{Endpoint: "get-agent", Err: fmt.Errorf("%w: agent %s is not a %s agent", ErrNotFound, id, typeName)}
	}
	a.fingerprint = metadata.Fingerprint
	a.id.Store(&id)
	return a, nil
}

// TypeName is the agent type this agent is an instance of.
func (a *Agent) TypeName() string { return a.typeName }

// AgentID returns the agent's ID. A client of [Upsert] asks the server for it
// the first time, and the server creates the agent when it is missing; the
// other clients already know it.
func (a *Agent) AgentID(ctx context.Context) (AgentID, error) {
	if id := a.id.Load(); id != nil {
		return *id, nil
	}
	ref, err := upsertAgent(ctx, a)
	if err != nil {
		return AgentID{}, err
	}
	a.id.Store(&ref.AgentID)
	return ref.AgentID, nil
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
