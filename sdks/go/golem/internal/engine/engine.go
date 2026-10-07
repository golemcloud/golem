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

// Package engine is the SDK's type system: how a Go type is described in a
// schema graph, how its values are encoded into and decoded from value trees,
// and the registered variants, enums, flag sets, unions and type-id pins that
// shape both. Every public package (agents, tools, reflection) builds on one
// [Engine], so a type means the same thing wherever it appears.
package engine

import (
	"fmt"
	"reflect"

	"github.com/golemcloud/golem/sdks/go/core/values"
)

// Definition errors are problems found while registering or deriving agent
// definitions — a bad Spec, an unsupported field type, a conflicting NameType,
// an invalid HTTP route. They are *collected*, not panicked.
//
// The reason is structural: agents are declared from package-level vars, so a
// panic fires during init() — before the host can call any export — and surfaces
// as an opaque wasm trap with no message. Instead the SDK records every such
// problem and reports it through the export channels that do carry an error:
// discover-agent-types (what the CLI calls at deploy) and initialize. This is
// the general form of the rule "anything we cannot catch at compile time must be
// checked at runtime and reported during discovery."

// DefError is one recorded problem, attributed as precisely as the point
// that found it allows.
type DefError struct {
	Agent  string // "" when not tied to a specific agent (e.g. NameType, a bad type)
	Method string // "" when not method-specific
	Detail string
}

func (e DefError) Error() string {
	switch {
	case e.Agent != "" && e.Method != "":
		return fmt.Sprintf("golem: agent %q method %q: %s", e.Agent, e.Method, e.Detail)
	case e.Agent != "":
		return fmt.Sprintf("golem: agent %q: %s", e.Agent, e.Detail)
	default:
		return "golem: " + e.Detail
	}
}

// Engine holds the registered variants, enums, flag sets, unions and name pins,
// the compiled codecs, and the definition errors found so far.
type Engine struct {
	Variants map[reflect.Type]*VariantDef
	Enums    map[reflect.Type]*EnumDef
	Flags    map[reflect.Type]*FlagsDef
	Unions   map[reflect.Type]*UnionDef
	Pins     map[reflect.Type]string // NameType type-id overrides
	Codecs   map[reflect.Type]*Codec // Compile memoization
	Errs     []DefError              // registration-phase errors (derivation adds more)

	// PrincipalType is the type the host fills from the invocation: a struct
	// field of it carries no value, and it is invalid anywhere else.
	PrincipalType reflect.Type
	// Composites recognise the SDK's own struct types that are not plain
	// records (secrets, streams, quota tokens, permission cards). Each fills c
	// and returns true when zero is one of its types.
	Composites []func(e *Engine, c *Codec, zero any) bool
}

// New returns an engine with the SDK's own types registered.
func New(principalType reflect.Type, composites ...func(e *Engine, c *Codec, zero any) bool) *Engine {
	e := &Engine{
		Variants:      map[reflect.Type]*VariantDef{},
		Enums:         map[reflect.Type]*EnumDef{},
		Flags:         map[reflect.Type]*FlagsDef{},
		Unions:        map[reflect.Type]*UnionDef{},
		Pins:          map[reflect.Type]string{},
		Codecs:        map[reflect.Type]*Codec{},
		PrincipalType: principalType,
		Composites:    composites,
	}
	// The modalities of a basic multimodal list are the SDK's own variant.
	e.DefineVariant(reflect.TypeFor[values.Modality](), []Case{
		{Name: "Text", Typ: reflect.TypeFor[values.TextModality](), Wrapped: true},
		{Name: "Binary", Typ: reflect.TypeFor[values.BinaryModality](), Wrapped: true},
	})
	return e
}

// RecordErr appends a registration-phase definition error. agent/method may be
// "" when the problem is not attributable to one (e.g. a conflicting NameType).
func (d *Engine) RecordErr(agent, method, format string, args ...any) {
	d.Errs = append(d.Errs, DefError{Agent: agent, Method: method, Detail: fmt.Sprintf(format, args...)})
}

// EncodeError marks an encode-stage panic caused by the agent supplying a value
// the wire cannot carry — a nil or unregistered variant case, an out-of-range
// enum value, or a Secret used as a parameter/return. It is the agent's mistake,
// so it is reported as an agent error rather than an INTERNAL SDK error.
type EncodeError struct{ Msg string }

func (e *EncodeError) Error() string { return e.Msg }
