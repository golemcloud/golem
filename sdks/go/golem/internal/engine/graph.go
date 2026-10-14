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

package engine

import (
	"reflect"
	"sort"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// The agent type carries ONE schema graph: a flat pool of type nodes, with
// constructor and method schemas referring to it by index. Schemas are derived
// from the Go types by reflection — this is what removes the need for a code
// generation step or an explicit schema DSL.

type GraphBuilder struct {
	Nodes []types.SchemaTypeNode
	// seen deduplicates by Go type, so a type used by several methods yields one
	// node. Sharing is legal: a consumer tracks cycles along the current path
	// only, so the same node reached twice by sibling paths is fine.
	seen map[reflect.Type]int32
	// refs maps a recursive type to its ref-type node, and defs holds the named
	// definitions those nodes point at.
	refs map[reflect.Type]int32
	defs []types.SchemaTypeDef
	// Invalids collects the types this graph references that could not be
	// compiled (see [Codec.Invalid]), so buildAgentType can attribute them to
	// the agent being built.
	Invalids map[reflect.Type]string
	// E is the definition set this graph is derived for; refNode reads its pins
	// to resolve type-ids.
	E *Engine
}

// Node returns the index of c's type Node, adding it if absent.
//
// The index is reserved and recorded *before* the body is built, because
// building it may recurse back into this same type. That reservation is what
// makes recursive types (a record reachable from its own fields) produce a
// finite graph instead of overflowing the stack.
func (g *GraphBuilder) Node(c *Codec) int32 {
	if c.Invalid != "" {
		if g.Invalids == nil {
			g.Invalids = map[reflect.Type]string{}
		}
		g.Invalids[c.Typ] = c.Invalid
	}
	if c.Recursive {
		return g.refNode(c)
	}
	if g.seen == nil {
		g.seen = map[reflect.Type]int32{}
	}
	if idx, ok := g.seen[c.Typ]; ok {
		return idx
	}
	idx := int32(len(g.Nodes))
	g.Nodes = append(g.Nodes, types.SchemaTypeNode{})
	g.seen[c.Typ] = idx

	// Sequenced deliberately: c.body may append nodes and reallocate g.nodes,
	// so the destination must be indexed only after it returns.
	body := c.Body(g)
	g.Nodes[idx].Body = body
	if c.Metadata != nil {
		g.Nodes[idx].Metadata = *c.Metadata
	}
	return idx
}

// refNode emits a recursive type as a named def plus the ref-type node that
// points at it. The ref node is registered before the body is built, so when the
// body reaches this type again it resolves to the ref instead of recursing —
// which is what keeps the flat node list acyclic.
func (g *GraphBuilder) refNode(c *Codec) int32 {
	if g.refs == nil {
		g.refs = map[reflect.Type]int32{}
	}
	if idx, ok := g.refs[c.Typ]; ok {
		return idx
	}

	defIdx := int32(len(g.defs))
	g.defs = append(g.defs, types.SchemaTypeDef{
		// Resolved here, at schema-build time (get-definition) — not at compile
		// time — so a NameType pin registered during package init is honored
		// regardless of whether the type's codec compiled first.
		Id:   g.E.TypeID(c.Typ),
		Name: witTypes.Some(c.Typ.String()),
	})

	refIdx := int32(len(g.Nodes))
	g.Nodes = append(g.Nodes, types.SchemaTypeNode{Body: types.MakeSchemaTypeBodyRefType(defIdx)})
	g.refs[c.Typ] = refIdx

	bodyIdx := int32(len(g.Nodes))
	g.Nodes = append(g.Nodes, types.SchemaTypeNode{})
	body := c.Body(g)
	g.Nodes[bodyIdx].Body = body
	g.defs[defIdx].Body = bodyIdx

	return refIdx
}

// sortDefs orders defs by id and rewrites the ref-type nodes accordingly, so a
// given set of Go types always produces a byte-identical graph.
func (g *GraphBuilder) sortDefs() {
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

	for i := range g.Nodes {
		if g.Nodes[i].Body.Tag() == types.SchemaTypeBodyRefType {
			g.Nodes[i].Body = types.MakeSchemaTypeBodyRefType(remap[g.Nodes[i].Body.RefType()])
		}
	}
}

func (g *GraphBuilder) Build() types.SchemaGraph {
	// schema.root is a structurally required placeholder, not the semantic root:
	// the meaningful roots are the per-parameter and per-output indices.
	if len(g.Nodes) == 0 {
		g.Nodes = append(g.Nodes, types.SchemaTypeNode{Body: types.MakeSchemaTypeBodyBoolType()})
	}
	g.sortDefs()
	return types.SchemaGraph{TypeNodes: g.Nodes, Defs: g.defs, Root: 0}
}

// CarriesStream reports whether a stream is reachable anywhere in t's schema,
// through named definitions and recursive types alike: an exact walk of the
// graph t publishes, so no cycle can hide one.
func (d *Engine) CarriesStream(t reflect.Type) bool {
	g := GraphBuilder{E: d}
	root := g.Node(d.Compile(t))
	graph := g.Build()
	seen := map[int32]bool{}
	var walk func(idx int32) bool
	walk = func(idx int32) bool {
		if idx < 0 || int(idx) >= len(graph.TypeNodes) || seen[idx] {
			return false
		}
		seen[idx] = true
		body := graph.TypeNodes[idx].Body
		some := func(o witTypes.Option[int32]) bool { return o.IsSome() && walk(o.Some()) }
		switch body.Tag() {
		case types.SchemaTypeBodyStreamType:
			return true
		case types.SchemaTypeBodyRefType:
			def := body.RefType()
			return int(def) < len(graph.Defs) && walk(graph.Defs[def].Body)
		case types.SchemaTypeBodyRecordType:
			for _, f := range body.RecordType() {
				if walk(f.Body) {
					return true
				}
			}
		case types.SchemaTypeBodyVariantType:
			for _, c := range body.VariantType() {
				if some(c.Payload) {
					return true
				}
			}
		case types.SchemaTypeBodyTupleType:
			for _, e := range body.TupleType() {
				if walk(e) {
					return true
				}
			}
		case types.SchemaTypeBodyListType:
			return walk(body.ListType())
		case types.SchemaTypeBodyFixedListType:
			return walk(body.FixedListType().Element)
		case types.SchemaTypeBodyMapType:
			return walk(body.MapType().Key) || walk(body.MapType().Value)
		case types.SchemaTypeBodyOptionType:
			return walk(body.OptionType())
		case types.SchemaTypeBodyResultType:
			return some(body.ResultType().Ok) || some(body.ResultType().Err)
		case types.SchemaTypeBodyUnionType:
			for _, b := range body.UnionType().Branches {
				if walk(b.Body) {
					return true
				}
			}
		case types.SchemaTypeBodySecretType:
			return walk(body.SecretType().Inner)
		case types.SchemaTypeBodyFutureType:
			return some(body.FutureType())
		}
		return false
	}
	return walk(root)
}

// GraphForType builds a standalone schema graph whose root is typ — the shape
// get-config-value and reveal expect for "expected".
func (d *Engine) GraphForType(typ reflect.Type) types.SchemaGraph {
	g := GraphBuilder{E: d}
	root := g.Node(d.Compile(typ))
	graph := g.Build()
	graph.Root = root
	return graph
}
