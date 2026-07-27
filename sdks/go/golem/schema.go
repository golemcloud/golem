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
	"sort"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// The agent type carries ONE schema graph: a flat pool of type nodes, with
// constructor and method schemas referring to it by index. Schemas are derived
// from the Go types by reflection — this is what removes the need for a code
// generation step or an explicit schema DSL.

type graphBuilder struct {
	nodes []types.SchemaTypeNode
	// seen deduplicates by Go type, so a type used by several methods yields one
	// node. Sharing is legal: a consumer tracks cycles along the current path
	// only, so the same node reached twice by sibling paths is fine.
	seen map[reflect.Type]int32
	// refs maps a recursive type to its ref-type node, and defs holds the named
	// definitions those nodes point at.
	refs map[reflect.Type]int32
	defs []types.SchemaTypeDef
}

// node returns the index of c's type node, adding it if absent.
//
// The index is reserved and recorded *before* the body is built, because
// building it may recurse back into this same type. That reservation is what
// makes recursive types (a record reachable from its own fields) produce a
// finite graph instead of overflowing the stack.
func (g *graphBuilder) node(c *codec) int32 {
	if c.recursive {
		return g.refNode(c)
	}
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

// refNode emits a recursive type as a named def plus the ref-type node that
// points at it. The ref node is registered before the body is built, so when the
// body reaches this type again it resolves to the ref instead of recursing —
// which is what keeps the flat node list acyclic.
func (g *graphBuilder) refNode(c *codec) int32 {
	if g.refs == nil {
		g.refs = map[reflect.Type]int32{}
	}
	if idx, ok := g.refs[c.typ]; ok {
		return idx
	}

	defIdx := int32(len(g.defs))
	g.defs = append(g.defs, types.SchemaTypeDef{
		// Resolved here, at schema-build time (get-definition) — not at compile
		// time — so a NameType pin registered during package init is honored
		// regardless of whether the type's codec compiled first.
		Id:   typeID(c.typ),
		Name: witTypes.Some(c.typ.String()),
	})

	refIdx := int32(len(g.nodes))
	g.nodes = append(g.nodes, types.SchemaTypeNode{Body: types.MakeSchemaTypeBodyRefType(defIdx)})
	g.refs[c.typ] = refIdx

	bodyIdx := int32(len(g.nodes))
	g.nodes = append(g.nodes, types.SchemaTypeNode{})
	body := c.body(g)
	g.nodes[bodyIdx].Body = body
	g.defs[defIdx].Body = bodyIdx

	return refIdx
}

// sortDefs orders defs by id and rewrites the ref-type nodes accordingly, so a
// given set of Go types always produces a byte-identical graph.
func (g *graphBuilder) sortDefs() {
	if len(g.defs) < 2 {
		return
	}
	order := make([]int, len(g.defs))
	for i := range order {
		order[i] = i
	}
	sort.Slice(order, func(a, b int) bool { return g.defs[order[a]].Id < g.defs[order[b]].Id })

	remap := make([]int32, len(g.defs))
	sorted := make([]types.SchemaTypeDef, len(g.defs))
	for newIdx, oldIdx := range order {
		sorted[newIdx] = g.defs[oldIdx]
		remap[oldIdx] = int32(newIdx)
	}
	g.defs = sorted

	for i := range g.nodes {
		if g.nodes[i].Body.Tag() == types.SchemaTypeBodyRefType {
			g.nodes[i].Body = types.MakeSchemaTypeBodyRefType(remap[g.nodes[i].Body.RefType()])
		}
	}
}

func (g *graphBuilder) build() types.SchemaGraph {
	// schema.root is a structurally required placeholder, not the semantic root:
	// the meaningful roots are the per-parameter and per-output indices.
	if len(g.nodes) == 0 {
		g.nodes = append(g.nodes, types.SchemaTypeNode{Body: types.MakeSchemaTypeBodyBoolType()})
	}
	g.sortDefs()
	return types.SchemaGraph{TypeNodes: g.nodes, Defs: g.defs, Root: 0}
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
