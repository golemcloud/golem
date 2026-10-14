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
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"reflect"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// namedFields turns a parameter list into WIT named-fields, adding each
// parameter's type to the shared graph.
func namedFields(g *engine.GraphBuilder, fs []engine.Field) []common.NamedField {
	out := make([]common.NamedField, 0, len(fs))
	for _, f := range fs {
		if f.AutoInjected {
			out = append(out, common.NamedField{
				Name:   f.Name,
				Source: common.MakeFieldSourceAutoInjected(common.AutoInjectedKindPrincipal),
				Schema: g.Node(principalSlotCodec),
			})
			continue
		}
		out = append(out, common.NamedField{
			Name:   f.Name,
			Source: common.MakeFieldSourceUserSupplied(),
			Schema: g.RestrictedNode(f.Codec, f.Restrict),
		})
	}
	return out
}

// principalSlotCodec types a principal field in the schema. The field carries
// no value, so like the Rust SDK it is an empty record; its own key type keeps
// it apart from a misplaced Principal's codec.
type principalSlot struct{}

var principalSlotCodec = &engine.Codec{
	Typ: reflect.TypeFor[principalSlot](),
	Body: func(*engine.GraphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{})
	},
}

// buildAgentType derives the full agent-type metadata reported by
// get-definition / discover-agent-types. The second result is the set of types
// referenced by this agent that could not be compiled, keyed by type — empty for
// a well-formed agent; finalize turns them into attributed definition errors.
func (d *definitions) buildAgentType(e *agentEntry) (common.AgentType, map[reflect.Type]string) {
	g := engine.GraphBuilder{E: d.Engine}

	ctorFields := namedFields(&g, e.idFields)
	methods := make([]common.AgentMethod, 0, len(e.order))
	for _, name := range e.order {
		m := e.methods[name]
		in := namedFields(&g, m.inFields)
		out := common.MakeOutputSchemaUnit()
		if m.outCodec != nil {
			out = common.MakeOutputSchemaSingle(g.Node(m.outCodec))
		}
		readOnly := witTypes.None[common.ReadOnlyConfig]()
		if m.readOnly != nil {
			readOnly = witTypes.Some(common.ReadOnlyConfig{
				CachePolicy:   m.readOnly.policy.toWit(),
				UsesPrincipal: len(principalFields(m.inFields)) > 0,
			})
		}
		methods = append(methods, common.AgentMethod{
			Name:         m.name,
			Description:  m.desc,
			InputSchema:  common.MakeInputSchemaParameters(in),
			OutputSchema: out,
			PromptHint:   someIfSet(m.hint),
			ReadOnly:     readOnly,
		})
	}

	// Compute config declarations before g.build(): each adds its value type to
	// the shared graph, and ValueType indexes into the built schema.
	configDecls := d.buildConfigDecls(&g, e.configs)

	at := common.AgentType{
		TypeName:       e.name,
		Kind:           e.kind(),
		Description:    e.desc,
		SourceLanguage: "go",
		Schema:         g.Build(),
		Constructor: common.AgentConstructor{
			Name:        witTypes.None[string](),
			Description: e.desc,
			PromptHint:  someIfSet(e.hint),
			InputSchema: common.MakeInputSchemaParameters(ctorFields),
		},
		Methods:      methods,
		Mode:         e.mode,
		HttpMount:    witTypes.None[common.HttpMountDetails](),
		Snapshotting: e.snapshot.toWit(),
		Config:       configDecls,
	}
	return at, g.Invalids
}

// kind is the agent type kind an entry publishes.
func (e *agentEntry) kind() common.AgentTypeKind {
	if e.router != nil {
		return common.AgentTypeKindHttpRouter
	}
	return common.AgentTypeKindRegular
}
