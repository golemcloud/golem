// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package golem

import (
	"reflect"

	common "github.com/golemcloud/golem-go/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem-go/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// The agent type carries ONE schema graph: a flat pool of type nodes, with
// constructor and method schemas referring to it by index. Schemas are derived
// from the Go types by reflection — this is what removes the need for a code
// generation step or an explicit schema DSL.

type graphBuilder struct {
	nodes []types.SchemaTypeNode
	// seen deduplicates by Go type, so a type used by several methods yields
	// one node — and, more importantly, so a self-referential type terminates.
	seen map[reflect.Type]int32
}

// node returns the index of c's type node, adding it if absent.
//
// The index is reserved and recorded *before* the body is built, because
// building it may recurse back into this same type. That reservation is what
// makes recursive types (a record reachable from its own fields) produce a
// finite graph instead of overflowing the stack.
func (g *graphBuilder) node(c *codec) int32 {
	if g.seen == nil {
		g.seen = map[reflect.Type]int32{}
	}
	if idx, ok := g.seen[c.typ]; ok {
		return idx
	}
	idx := int32(len(g.nodes))
	g.nodes = append(g.nodes, types.SchemaTypeNode{})
	g.seen[c.typ] = idx

	// Sequenced deliberately: c.body may append nodes and reallocate g.nodes,
	// so the destination must be indexed only after it returns.
	body := c.body(g)
	g.nodes[idx].Body = body
	return idx
}

func (g *graphBuilder) build() types.SchemaGraph {
	// schema.root is a structurally required placeholder, not the semantic root:
	// the meaningful roots are the per-parameter and per-output indices.
	if len(g.nodes) == 0 {
		g.nodes = append(g.nodes, types.SchemaTypeNode{Body: types.MakeSchemaTypeBodyBoolType()})
	}
	return types.SchemaGraph{TypeNodes: g.nodes, Root: 0}
}

// namedFields turns a parameter list into WIT named-fields, adding each
// parameter's type to the shared graph.
func namedFields(g *graphBuilder, fs []fieldInfo) []common.NamedField {
	out := make([]common.NamedField, 0, len(fs))
	for _, f := range fs {
		out = append(out, common.NamedField{
			Name:   f.name,
			Source: common.MakeFieldSourceUserSupplied(),
			Schema: g.node(f.codec),
		})
	}
	return out
}

// buildAgentType derives the full agent-type metadata reported by
// get-definition / discover-agent-types.
func buildAgentType(e *agentEntry) common.AgentType {
	var g graphBuilder

	ctorFields := namedFields(&g, e.idFields)
	methods := make([]common.AgentMethod, 0, len(e.order))
	for _, name := range e.order {
		m := e.methods[name]
		in := namedFields(&g, m.inFields)
		out := common.MakeOutputSchemaUnit()
		if m.outCodec != nil {
			out = common.MakeOutputSchemaSingle(g.node(m.outCodec))
		}
		methods = append(methods, common.AgentMethod{
			Name:         m.name,
			Description:  m.desc,
			InputSchema:  common.MakeInputSchemaParameters(in),
			OutputSchema: out,
			PromptHint:   witTypes.None[string](),
			ReadOnly:     witTypes.None[common.ReadOnlyConfig](),
		})
	}

	return common.AgentType{
		TypeName:       e.name,
		Description:    e.desc,
		SourceLanguage: "go",
		Schema:         g.build(),
		Constructor: common.AgentConstructor{
			Name:        witTypes.None[string](),
			Description: e.desc,
			PromptHint:  witTypes.None[string](),
			InputSchema: common.MakeInputSchemaParameters(ctorFields),
		},
		Methods:      methods,
		Mode:         e.mode,
		HttpMount:    witTypes.None[common.HttpMountDetails](),
		Snapshotting: common.MakeSnapshottingDisabled(),
	}
}
