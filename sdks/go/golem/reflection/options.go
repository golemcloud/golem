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

package reflection

import (
	"slices"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// ClientOpt configures a reflected client.
type ClientOpt func(*clientOpts)

type clientOpts struct {
	phantomID witTypes.Option[types.Uuid]
	overrides []configOverride
}

//nolint:unused // called from host_wasm.go
func applyOpts(opts []ClientOpt) clientOpts {
	o := clientOpts{phantomID: witTypes.None[types.Uuid]()}
	for _, opt := range opts {
		opt(&o)
	}
	return o
}

// WithPhantomID addresses a specific phantom instance of the target agent.
// Use [AgentType.NewPhantom] to allocate a fresh one.
func WithPhantomID(id golem.UUID) ClientOpt {
	return func(o *clientOpts) { o.phantomID = witTypes.Some(engine.UUIDToWit(id)) }
}

// WithConfigJSON supplies one local config value of the target at creation,
// as canonical JSON for the declared path. It is checked against the
// snapshot's declaration before anything is sent.
func WithConfigJSON(path []string, value any) ClientOpt {
	return func(o *clientOpts) {
		o.overrides = append(o.overrides, configOverride{path: slices.Clone(path), json: value})
	}
}

// WithConfigValue is [WithConfigJSON] for a value already in the schema model.
func WithConfigValue(path []string, value core.SchemaValue) ClientOpt {
	return func(o *clientOpts) {
		o.overrides = append(o.overrides, configOverride{path: slices.Clone(path), value: value, native: true})
	}
}

// configOverride is one untyped configuration entry: canonical JSON, or a
// schema-model value when native.
type configOverride struct {
	path   []string
	json   any
	value  core.SchemaValue
	native bool
}
