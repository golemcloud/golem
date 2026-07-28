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
	"fmt"
	"reflect"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	reveal "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_secrets_reveal"
	secrets "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_secrets_types"
)

// Config and secrets.
//
// An agent declares the config keys and secrets it needs; the platform
// provisions the values (golem.yaml config/env/envDefaults/secretDefaults + the
// CLI), and the guest reads them at runtime. A declaration is a package-level,
// typed descriptor tied to one agent — mirroring how methods are descriptors:
//
//	var Greeting = golem.DefineConfig[string](Counter, "greeting")
//	var APIKey   = golem.DefineSecret[string](Counter, "api", "key")
//
// and read inside an invocation:
//
//	g := Greeting.Get()          // (string, error)
//	k := APIKey.Get()            // (golem.Secret[string], error); k.Reveal() is the plaintext
//
// Both flow through golem:agent/host.get-config-value, keyed by the multi-segment
// path, and access is gated by these declarations: reading an *undeclared* key
// traps. A required key that is unset also traps — declare the value type as a
// pointer (e.g. DefineConfig[*string]) to read it back as nil instead.
//
// The wire type-name distinction between a plain value and a secret is the schema
// node: a secret's value type is secret(inner). A secret read returns an opaque
// handle that must be revealed through golem:secrets/reveal.

// configDecl is one registered config/secret key on an agent. For a secret, typ
// is Secret[T] so the schema graph carries a secret(inner) node; for local
// config it is the value type directly.
type configDecl struct {
	source common.AgentConfigSource
	path   []string
	typ    reflect.Type
}

// Config is a declared local configuration key of value type T, tied to one
// agent. Read its value inside an invocation with [Config.Get].
type Config[T any] struct {
	path []string
}

// SecretConfig is a declared secret whose revealed value has type T, tied to one
// agent. Read the plaintext with [SecretConfig.Get] or obtain an unrevealed
// handle with [SecretConfig.Handle].
type SecretConfig[T any] struct {
	path []string
}

// SecretHandle references a secret's material without revealing it, so it can be
// handed to host capabilities (host-mediated substitution) without the guest
// ever seeing the plaintext.
type SecretHandle struct {
	secret *types.Secret
}

// DefineConfig declares a local configuration key of value type T on the agent,
// addressed by the given path. Use a pointer value type for an optional key.
func DefineConfig[T any, Id any, S any](a *Agent[Id, S], path ...string) *Config[T] {
	return defineConfigInto[T](defs, a, path)
}

// DefineSecret declares a secret on the agent whose revealed value has type T,
// addressed by the given path.
func DefineSecret[T any, Id any, S any](a *Agent[Id, S], path ...string) *SecretConfig[T] {
	return defineSecretInto[T](defs, a, path)
}

func defineConfigInto[T any, Id any, S any](d *definitions, a *Agent[Id, S], path []string) *Config[T] {
	recordConfigDecl(d, a, common.AgentConfigSourceLocal, path, reflect.TypeFor[T]())
	return &Config[T]{path: clonePath(path)}
}

func defineSecretInto[T any, Id any, S any](d *definitions, a *Agent[Id, S], path []string) *SecretConfig[T] {
	// Store the Secret[T] type so the derived schema node is secret(inner).
	recordConfigDecl(d, a, common.AgentConfigSourceSecret, path, reflect.TypeFor[Secret[T]]())
	return &SecretConfig[T]{path: clonePath(path)}
}

// recordConfigDecl validates a declaration and records it on the agent, or
// collects an attributed definition error. Anything not checkable at compile
// time is reported here and surfaced at discovery, never panicked.
func recordConfigDecl[Id any, S any](d *definitions, a *Agent[Id, S], source common.AgentConfigSource, path []string, typ reflect.Type) {
	kind := "config"
	if source == common.AgentConfigSourceSecret {
		kind = "secret"
	}

	if a == nil {
		d.recordErr("", "", "%s %v: declared on a nil agent", kind, path)
		return
	}
	e := d.agents[a.name]
	if e == nil {
		d.recordErr(a.name, "", "%s %v: unknown agent %q", kind, path, a.name)
		return
	}
	if len(path) == 0 {
		d.recordErr(a.name, "", "%s: declared with an empty path", kind)
		return
	}
	for _, seg := range path {
		if seg == "" {
			d.recordErr(a.name, "", "%s %v: path has an empty segment", kind, path)
			return
		}
	}
	for _, cd := range e.configs {
		if pathsEqual(cd.path, path) {
			d.recordErr(a.name, "", "config path %v declared more than once", path)
			return
		}
	}
	e.configs = append(e.configs, configDecl{source: source, path: clonePath(path), typ: typ})
}

// buildConfigDecls derives the agent-type config metadata, compiling each key's
// value type into the shared graph g (so ValueType is an index into the agent's
// schema). An uncompilable type lands in g.invalids and is attributed by
// discover, exactly like a method parameter.
func (d *definitions) buildConfigDecls(g *graphBuilder, cds []configDecl) []common.AgentConfigDeclaration {
	if len(cds) == 0 {
		return nil
	}
	out := make([]common.AgentConfigDeclaration, 0, len(cds))
	for _, cd := range cds {
		out = append(out, common.AgentConfigDeclaration{
			Source:    cd.source,
			Path:      cd.path,
			ValueType: g.node(d.compile(cd.typ)),
		})
	}
	return out
}

// ---------------------------------------------------------------------------
// runtime reads — the host calls are wasm-only; the graph build and decode are
// pure and covered by native tests.
// ---------------------------------------------------------------------------

// Get reads the current value of the config key.
func (c *Config[T]) Get() (T, error) {
	tree := host.GetConfigValue(c.path, defs.graphForType(reflect.TypeFor[T]()))
	return decodeConfigValue[T](defs, c.path, tree)
}

// Get reveals and decodes the secret's current value, wrapped in a [Secret] so
// it stays redacted in logs; call Reveal on the result for the plaintext.
func (s *SecretConfig[T]) Get() (Secret[T], error) {
	var zero Secret[T]
	handle, err := s.resolve()
	if err != nil {
		return zero, err
	}
	res := reveal.Reveal(handle, defs.graphForType(reflect.TypeFor[T]()))
	if res.IsErr() {
		return zero, secretErrorToGo(s.path, res.Err())
	}
	val, err := decodeConfigValue[T](defs, s.path, res.Ok())
	if err != nil {
		return zero, err
	}
	return NewSecret(val), nil
}

// Handle returns the secret as an unrevealed handle, for passing to host
// capabilities without the guest observing the plaintext.
func (s *SecretConfig[T]) Handle() (SecretHandle, error) {
	handle, err := s.resolve()
	if err != nil {
		return SecretHandle{}, err
	}
	return SecretHandle{secret: handle}, nil
}

// resolve fetches the secret handle from the host (get-config-value against a
// secret(inner) schema).
func (s *SecretConfig[T]) resolve() (*types.Secret, error) {
	tree := host.GetConfigValue(s.path, defs.graphForType(reflect.TypeFor[Secret[T]]()))
	return extractSecretHandle(s.path, tree)
}

// graphForType builds a standalone schema graph whose root is typ — the shape
// get-config-value and reveal expect for "expected".
func (d *definitions) graphForType(typ reflect.Type) types.SchemaGraph {
	g := graphBuilder{d: d}
	root := g.node(d.compile(typ))
	graph := g.build()
	graph.Root = root
	return graph
}

// decodeConfigValue decodes a config value tree into T through T's codec. Pure.
func decodeConfigValue[T any](d *definitions, path []string, tree types.SchemaValueTree) (T, error) {
	var zero T
	typ := reflect.TypeFor[T]()
	dst := reflect.New(typ).Elem()
	dec := decoder{nodes: tree.ValueNodes}
	if err := d.compile(typ).decode(&dec, dst, tree.Root); err != nil {
		return zero, fmt.Errorf("golem/config %v: %w", path, err)
	}
	return dst.Interface().(T), nil
}

// extractSecretHandle pulls the secret handle out of a get-config-value result.
// Pure — the value tree carries the handle at its root as a secret value node.
func extractSecretHandle(path []string, tree types.SchemaValueTree) (*types.Secret, error) {
	if tree.Root < 0 || int(tree.Root) >= len(tree.ValueNodes) {
		return nil, fmt.Errorf("golem/secret %v: empty config value tree", path)
	}
	node := tree.ValueNodes[tree.Root]
	if node.Tag() != types.SchemaValueNodeSecretValue {
		return nil, fmt.Errorf("golem/secret %v: expected a secret value, got node tag %d", path, node.Tag())
	}
	return node.SecretValue(), nil
}

// secretErrorToGo maps a host secret-error onto a Go error, keeping the case
// distinguishable rather than flattening it to a bare string.
func secretErrorToGo(path []string, e secrets.SecretError) error {
	switch e.Tag() {
	case secrets.SecretErrorUnavailable:
		return fmt.Errorf("golem/secret %v: unavailable: %s", path, e.Unavailable())
	case secrets.SecretErrorVersionNotFound:
		return fmt.Errorf("golem/secret %v: requested version not found", path)
	case secrets.SecretErrorInternal:
		return fmt.Errorf("golem/secret %v: internal error: %s", path, e.Internal())
	default:
		return fmt.Errorf("golem/secret %v: secret error (tag %d)", path, e.Tag())
	}
}

// ---------------------------------------------------------------------------
// config-on-RPC: a caller can override a callee's local config values at client
// creation. The value is encoded here (pure) and threaded into make-wasm-rpc by
// ClientFor.
// ---------------------------------------------------------------------------

// configOverrideFn produces one config override, deferred so the value is
// encoded against the definition set at client-creation time.
type configOverrideFn func(d *definitions) (common.TypedAgentConfigValue, error)

// WithConfigValue overrides a local config value on the target agent at client
// creation (make-wasm-rpc's agent-config). The value type must match the key's,
// and the key must be one the target agent declares — otherwise ClientFor fails.
// Secrets are provisioned by the platform, not passed here, so only local
// [Config] keys are overridable.
func WithConfigValue[T any](key *Config[T], value T) ClientOpt {
	return func(o *clientOpts) {
		o.configs = append(o.configs, func(d *definitions) (common.TypedAgentConfigValue, error) {
			if key == nil {
				return common.TypedAgentConfigValue{}, fmt.Errorf("WithConfigValue: nil config key")
			}
			tv, err := encodeTypedValue(d, value)
			if err != nil {
				return common.TypedAgentConfigValue{}, err
			}
			return common.TypedAgentConfigValue{Path: clonePath(key.path), Value: tv}, nil
		})
	}
}

// buildAgentConfig encodes and validates the client's config overrides against
// the target agent's declarations. Pure — the encode and the declared-key check
// are both native-testable; ClientFor supplies the live definition set.
func buildAgentConfig(d *definitions, e *agentEntry, overrides []configOverrideFn) ([]common.TypedAgentConfigValue, error) {
	if len(overrides) == 0 {
		return nil, nil
	}
	out := make([]common.TypedAgentConfigValue, 0, len(overrides))
	for _, fn := range overrides {
		tv, err := fn(d)
		if err != nil {
			return nil, err
		}
		if !configDeclared(e, tv.Path) {
			return nil, fmt.Errorf("config override %v is not a declared config key on the agent", tv.Path)
		}
		out = append(out, tv)
	}
	return out, nil
}

func configDeclared(e *agentEntry, path []string) bool {
	for _, cd := range e.configs {
		if pathsEqual(cd.path, path) {
			return true
		}
	}
	return false
}

// encodeTypedValue encodes a Go value into a typed schema value (graph + value
// tree) for the wire. Pure.
func encodeTypedValue[T any](d *definitions, value T) (tv types.TypedSchemaValue, err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("encoding config value: %v", r)
		}
	}()
	typ := reflect.TypeFor[T]()
	return types.TypedSchemaValue{
		Graph: d.graphForType(typ),
		Value: encodeWith(d.compile(typ), reflect.ValueOf(value)),
	}, nil
}

func pathsEqual(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

func clonePath(p []string) []string { return append([]string(nil), p...) }
