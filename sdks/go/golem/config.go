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
// Config is attached to an agent as a single struct type and read only from
// within that agent's own execution — its methods and its constructor — for that
// agent's config type. This mirrors the TS SDK (config is instance-bound); there
// are no free-floating, read-from-anywhere descriptors.
//
// Declare the config struct and attach it to the agent:
//
//	type ShopConfig struct {
//	    Greeting string
//	    APIKey   golem.Secret[string] // a secret field, at any depth
//	}
//	var ShopCfg = golem.DefineConfig[ShopConfig](Shop)
//
// and read it from inside a method (or the constructor) via the agent's context:
//
//	cfg := ShopCfg.Get(ctx)  // ctx is the running agent's *Context[S]
//	_ = cfg.Greeting         // local field, decoded at this Get
//	_ = cfg.APIKey.Get()     // secret field: re-reads the host each call
//
// Local fields are the values current at the ShopCfg.Get(ctx) call. Secret fields
// come back as handles that read the host on every Secret.Get(), so a *rotated*
// secret is always observed rather than a stale snapshot.
//
// Each exported field of the config struct becomes a config key; nested structs
// flatten into multi-segment paths (field names lower-cased to match
// record-field naming), and any [Secret]-typed field becomes a secret. The
// platform provisions the values (golem.yaml config/env/envDefaults/
// secretDefaults + the CLI); the guest reads them at runtime through
// golem:agent/host.get-config-value, keyed by the multi-segment path. Access is
// gated by these declarations: reading an *undeclared* key traps, as does an
// unset required key — declare a field's type as a pointer to read it back as
// nil instead.
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

// NoConfig is the empty config type used by an agent that declares no config. It
// has no fields, so it flattens to zero config declarations. [DefineAgent] uses
// it internally; config-less agents never name it.
type NoConfig struct{}

// AgentConfig is the handle to an agent's configuration, returned by
// [DefineConfig] and [DefineConfiguredAgent]. It carries the agent's state type
// S and its config struct type Cfg, so the config can be read only from within
// that agent's own execution scope (see [AgentConfig.Get]) — never free-floating.
type AgentConfig[S any, Cfg any] struct {
	agentName string
}

// DefineConfig attaches config type Cfg to an agent declared with [DefineAgent]
// and returns a handle for reading it. Cfg's exported fields become the agent's
// config keys (nested structs flatten into multi-segment paths; [Secret]-typed
// fields at any depth become secrets). Read the values from within the agent's
// methods via [AgentConfig.Get].
//
// When the constructor itself needs config, declare the agent with
// [DefineConfiguredAgent] instead, which returns both the agent and its config
// handle.
func DefineConfig[Cfg any, Id any, S any](a *Agent[Id, S]) *AgentConfig[S, Cfg] {
	return defineConfigInto[Cfg](defs, a)
}

func defineConfigInto[Cfg any, Id any, S any](d *definitions, a *Agent[Id, S]) *AgentConfig[S, Cfg] {
	if a == nil {
		d.recordErr("", "", "DefineConfig: declared on a nil agent")
		return &AgentConfig[S, Cfg]{}
	}
	e := d.agents[a.name]
	if e == nil {
		d.recordErr(a.name, "", "DefineConfig: unknown agent %q (was DefineAgent called?)", a.name)
		return &AgentConfig[S, Cfg]{agentName: a.name}
	}
	flattenConfigStruct(d, e, a.name, reflect.TypeFor[Cfg]())
	return &AgentConfig[S, Cfg]{agentName: a.name}
}

func configKind(source common.AgentConfigSource) string {
	if source == common.AgentConfigSourceSecret {
		return "secret"
	}
	return "config"
}

// recordConfigOn validates a single declaration against an already-resolved
// entry and records it. Shared by every declaration path (the flattened config
// struct).
func recordConfigOn(d *definitions, e *agentEntry, agentName string, source common.AgentConfigSource, path []string, typ reflect.Type) {
	kind := configKind(source)
	if len(path) == 0 {
		d.recordErr(agentName, "", "%s: declared with an empty path", kind)
		return
	}
	for _, seg := range path {
		if seg == "" {
			d.recordErr(agentName, "", "%s %v: path has an empty segment", kind, path)
			return
		}
	}
	for _, cd := range e.configs {
		if pathsEqual(cd.path, path) {
			d.recordErr(agentName, "", "config path %v declared more than once", path)
			return
		}
	}
	e.configs = append(e.configs, configDecl{source: source, path: clonePath(path), typ: typ})
}

// ---------------------------------------------------------------------------
// struct/record flattening: an agent's whole config surface is one Go struct.
// configLeaves flattens it into per-key declarations, and materializeConfig
// reads it back into a struct value.
// ---------------------------------------------------------------------------

// configLeaf is one flattened config key discovered in a struct: its source,
// wire path, Go type, and the field-index path to set it during materialize.
type configLeaf struct {
	source common.AgentConfigSource
	path   []string
	typ    reflect.Type
	index  []int
}

var secretishType = reflect.TypeOf((*secretish)(nil)).Elem()

// isSecretType reports whether t is a golem.Secret[...] (the only type that
// implements the unexported secret interface), which flattens as a secret leaf.
func isSecretType(t reflect.Type) bool { return t.Implements(secretishType) }

// configLeaves flattens a config struct into its leaves. A Secret field is a
// secret leaf; a plain (non-secret) struct field recurses; everything else is a
// local leaf. Pure — shared by declaration (flattenConfigStruct) and read
// (materializeConfig).
func configLeaves(cfgType reflect.Type) ([]configLeaf, error) {
	if cfgType.Kind() != reflect.Struct {
		return nil, fmt.Errorf("config type must be a struct, got %s", cfgType)
	}
	var leaves []configLeaf
	var walk func(prefix []string, idx []int, t reflect.Type)
	walk = func(prefix []string, idx []int, t reflect.Type) {
		for i := 0; i < t.NumField(); i++ {
			f := t.Field(i)
			if !f.IsExported() {
				continue
			}
			path := append(clonePath(prefix), lowerFirst(f.Name))
			index := append(append([]int(nil), idx...), i)
			switch {
			case isSecretType(f.Type):
				leaves = append(leaves, configLeaf{common.AgentConfigSourceSecret, path, f.Type, index})
			case f.Type.Kind() == reflect.Struct:
				walk(path, index, f.Type)
			default:
				leaves = append(leaves, configLeaf{common.AgentConfigSourceLocal, path, f.Type, index})
			}
		}
	}
	walk(nil, nil, cfgType)
	return leaves, nil
}

// flattenConfigStruct records every leaf of a config struct on the entry. The
// empty [NoConfig] struct yields no leaves and records nothing.
func flattenConfigStruct(d *definitions, e *agentEntry, agentName string, cfgType reflect.Type) {
	leaves, err := configLeaves(cfgType)
	if err != nil {
		d.recordErr(agentName, "", "config: %v", err)
		return
	}
	for _, lf := range leaves {
		recordConfigOn(d, e, agentName, lf.source, lf.path, lf.typ)
	}
}

// ---------------------------------------------------------------------------
// runtime reads — the host calls are wasm-only; the graph build and decode are
// pure and covered by native tests.
// ---------------------------------------------------------------------------

// Get materializes the agent's config from within a method, returning it or
// panicking if the read fails. scope is the running agent's *[Context]: local
// fields are decoded, and secret fields come back as redacting [Secret] handles.
// Because scope must carry the agent's own state type S, the config cannot be
// read outside the owning agent — there is no free-floating read. A config read
// failing is a hard failure (a misconfiguration or host error) with no in-band
// recovery, so — like the TS/Rust/Scala SDKs — Get fails loud: the panic is
// recovered by the invoke dispatcher into an agent-error. Must be called inside
// an invocation (it calls the host).
func (c *AgentConfig[S, Cfg]) Get(scope agentScope[S]) Cfg {
	// scope is a compile-time gate only: requiring it means the read can happen
	// only from inside the agent's own execution. The host keys get-config-value
	// by the running agent, so the value is resolved without threading scope on.
	_ = scope
	cfg, err := materializeConfig[Cfg](func(lf configLeaf) (reflect.Value, error) {
		return readConfigLeaf(defs, lf)
	})
	if err != nil {
		panic(err)
	}
	return cfg
}

// Config reads the agent's config from within its constructor, returning it or
// panicking if the read fails (fail-loud, like [AgentConfig.Get]). It
// materializes the same Cfg: local fields decoded, secret fields as redacting
// [Secret] handles. The constructor reads config this way — off its own context —
// rather than through the config handle, which would be a self-reference in the
// package-level var that produces both the agent and the handle. Must be called
// inside an invocation (it calls the host).
func (c *InitContext[Id, S, Cfg]) Config() Cfg {
	cfg, err := materializeConfig[Cfg](func(lf configLeaf) (reflect.Value, error) {
		return readConfigLeaf(defs, lf)
	})
	if err != nil {
		panic(err)
	}
	return cfg
}

// materializeConfig assembles a Cfg value by reading each declared leaf through
// readLeaf. Pure given readLeaf — [AgentConfig.Get] supplies the host-backed
// reader, tests supply a fake.
func materializeConfig[Cfg any](readLeaf func(lf configLeaf) (reflect.Value, error)) (Cfg, error) {
	var zero Cfg
	cfgType := reflect.TypeFor[Cfg]()
	leaves, err := configLeaves(cfgType)
	if err != nil {
		return zero, err
	}
	out := reflect.New(cfgType).Elem()
	for _, lf := range leaves {
		v, err := readLeaf(lf)
		if err != nil {
			return zero, err
		}
		out.FieldByIndex(lf.index).Set(v)
	}
	return out.Interface().(Cfg), nil
}

// readConfigLeaf reads one leaf from the host: a local value is decoded directly;
// a secret is fetched, revealed, and rebuilt as a Secret[T].
func readConfigLeaf(d *definitions, lf configLeaf) (reflect.Value, error) {
	if lf.source == common.AgentConfigSourceSecret {
		return readSecretLeaf(lf)
	}
	tree := host.GetConfigValue(lf.path, d.graphForType(lf.typ))
	dst := reflect.New(lf.typ).Elem()
	dec := decoder{nodes: tree.ValueNodes}
	if err := d.compile(lf.typ).decode(&dec, dst, tree.Root); err != nil {
		return reflect.Value{}, fmt.Errorf("golem/config %v: %w", lf.path, err)
	}
	return dst, nil
}

// readSecretLeaf builds the Secret[T] handle for a config secret leaf. It does
// NOT touch the host here: the handle re-reads the current value lazily on each
// [Secret.Get], so a rotated secret is observed. The bind is done by reflection
// because the inner T is not statically known at this point. Keeping the host
// call out of this path (it lives in the Secret's closure, reachable only from a
// user's Secret.Get) is what lets native tests link.
func readSecretLeaf(lf configLeaf) (reflect.Value, error) {
	ptr := reflect.New(lf.typ) // *Secret[T]
	ptr.Interface().(secretBinder).secretBindPath(clonePath(lf.path))
	return ptr.Elem(), nil
}

// readSecretValue fetches, reveals, and decodes a config secret's CURRENT value.
// [Secret.Get] calls it on every access, so each read returns the latest value
// (a fresh get-config-value mints a handle pinned to the current revision, which
// reveal then unpacks). Host-backed — reachable only from Secret.Get, never from
// the config-materialization path.
func readSecretValue[T any](d *definitions, path []string) (T, error) {
	var zero T
	innerType := reflect.TypeFor[T]()

	handleTree := host.GetConfigValue(path, d.graphForType(reflect.TypeFor[Secret[T]]()))
	handle, err := extractSecretHandle(path, handleTree)
	if err != nil {
		return zero, err
	}
	res := reveal.Reveal(handle, d.graphForType(innerType))
	if res.IsErr() {
		return zero, secretErrorToGo(path, res.Err())
	}

	dst := reflect.New(innerType).Elem()
	dec := decoder{nodes: res.Ok().ValueNodes}
	if err := d.compile(innerType).decode(&dec, dst, res.Ok().Root); err != nil {
		return zero, fmt.Errorf("golem/secret %v: %w", path, err)
	}
	return dst.Interface().(T), nil
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
// creation. The values are encoded here (pure) and threaded into make-wasm-rpc
// by ClientFor.
// ---------------------------------------------------------------------------

// configOverrideFn produces the config overrides for one WithConfig option,
// deferred so the values are encoded against the definition set at
// client-creation time.
type configOverrideFn func(d *definitions) ([]common.TypedAgentConfigValue, error)

// WithConfig supplies the callee's local config values at client creation
// (make-wasm-rpc's agent-config), overriding what the platform would otherwise
// provision. Pass the target agent's config handle and a Cfg value; each local
// field is encoded as an override, validated against the target's declarations
// by [ClientFor]. Secret fields are provisioned by the platform, not by a
// caller, so they are skipped.
func WithConfig[S any, Cfg any](handle *AgentConfig[S, Cfg], value Cfg) ClientOpt {
	return func(o *clientOpts) {
		o.configs = append(o.configs, func(d *definitions) ([]common.TypedAgentConfigValue, error) {
			if handle == nil {
				return nil, fmt.Errorf("WithConfig: nil config handle")
			}
			return encodeConfigOverrides(d, value)
		})
	}
}

// encodeConfigOverrides encodes each local leaf of a config value into a typed
// override. Secret leaves are platform-provisioned, so they are skipped rather
// than sent. Pure.
func encodeConfigOverrides[Cfg any](d *definitions, value Cfg) ([]common.TypedAgentConfigValue, error) {
	leaves, err := configLeaves(reflect.TypeFor[Cfg]())
	if err != nil {
		return nil, err
	}
	root := reflect.ValueOf(value)
	var out []common.TypedAgentConfigValue
	for _, lf := range leaves {
		if lf.source == common.AgentConfigSourceSecret {
			continue
		}
		tv, err := encodeReflectValue(d, root.FieldByIndex(lf.index))
		if err != nil {
			return nil, fmt.Errorf("config override %v: %w", lf.path, err)
		}
		out = append(out, common.TypedAgentConfigValue{Path: clonePath(lf.path), Value: tv})
	}
	return out, nil
}

// buildAgentConfig encodes and validates the client's config overrides against
// the target agent's declarations. Pure — the encode and the declared-key check
// are both native-testable; ClientFor supplies the live definition set.
func buildAgentConfig(d *definitions, e *agentEntry, overrides []configOverrideFn) ([]common.TypedAgentConfigValue, error) {
	if len(overrides) == 0 {
		return nil, nil
	}
	var out []common.TypedAgentConfigValue
	for _, fn := range overrides {
		tvs, err := fn(d)
		if err != nil {
			return nil, err
		}
		for _, tv := range tvs {
			if !configDeclared(e, tv.Path) {
				return nil, fmt.Errorf("config override %v is not a declared config key on the agent", tv.Path)
			}
			out = append(out, tv)
		}
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

// encodeReflectValue encodes a Go value (as a reflect.Value) into a typed schema
// value (graph + value tree) for the wire. Pure.
func encodeReflectValue(d *definitions, v reflect.Value) (tv types.TypedSchemaValue, err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("encoding config value: %v", r)
		}
	}()
	typ := v.Type()
	return types.TypedSchemaValue{
		Graph: d.graphForType(typ),
		Value: encodeWith(d.compile(typ), v),
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
