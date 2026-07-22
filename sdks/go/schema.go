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
	"fmt"
	"reflect"

	common "github.com/golemcloud/golem-go/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem-go/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// The agent type carries ONE schema graph: a flat pool of type nodes, with
// constructor and method schemas referring to it by index. Schemas are derived
// from the Go types by reflection — this is what removes the need for a code
// generation step or an explicit schema DSL.

type graphBuilder struct{ nodes []types.SchemaTypeNode }

func (g *graphBuilder) push(body types.SchemaTypeBody) int32 {
	g.nodes = append(g.nodes, types.SchemaTypeNode{Body: body})
	return int32(len(g.nodes) - 1)
}

// add appends the nodes describing t and returns the index of its root node.
func (g *graphBuilder) add(t reflect.Type) int32 {
	// Records: children first, then the record node referring to them.
	if t.Kind() == reflect.Struct && t != reflect.TypeFor[Unit]() {
		var fields []types.NamedFieldType
		for i := range t.NumField() {
			f := t.Field(i)
			if f.PkgPath != "" {
				continue
			}
			fields = append(fields, types.NamedFieldType{Name: lowerFirst(f.Name), Body: g.add(f.Type)})
		}
		return g.push(types.MakeSchemaTypeBodyRecordType(fields))
	}
	return g.push(bodyFor(t))
}

func (g *graphBuilder) build() types.SchemaGraph {
	// schema.root is a structurally required placeholder, not the semantic root:
	// the meaningful roots are the per-parameter and per-output indices.
	if len(g.nodes) == 0 {
		g.push(types.MakeSchemaTypeBodyBoolType())
	}
	return types.SchemaGraph{TypeNodes: g.nodes, Root: 0}
}

// bodyFor maps a Go scalar type onto a WIT type body. Bare int/uint are
// rejected: their width is platform-dependent, so the wire type would be
// ambiguous.
func bodyFor(t reflect.Type) types.SchemaTypeBody {
	none := witTypes.None[types.NumericRestrictions]()
	switch t.Kind() {
	case reflect.String:
		return types.MakeSchemaTypeBodyStringType()
	case reflect.Bool:
		return types.MakeSchemaTypeBodyBoolType()
	case reflect.Int64:
		return types.MakeSchemaTypeBodyS64Type(none)
	case reflect.Int32:
		return types.MakeSchemaTypeBodyS32Type(none)
	case reflect.Int16:
		return types.MakeSchemaTypeBodyS16Type(none)
	case reflect.Int8:
		return types.MakeSchemaTypeBodyS8Type(none)
	case reflect.Uint64:
		return types.MakeSchemaTypeBodyU64Type(none)
	case reflect.Uint32:
		return types.MakeSchemaTypeBodyU32Type(none)
	case reflect.Uint16:
		return types.MakeSchemaTypeBodyU16Type(none)
	case reflect.Uint8:
		return types.MakeSchemaTypeBodyU8Type(none)
	case reflect.Float64:
		return types.MakeSchemaTypeBodyF64Type(none)
	case reflect.Float32:
		return types.MakeSchemaTypeBodyF32Type(none)
	case reflect.Int, reflect.Uint:
		panic(fmt.Sprintf("golem: %s has a platform-dependent width; use a sized type such as int64/uint64", t))
	default:
		panic(fmt.Sprintf("golem: unsupported type %s (kind %s)", t, t.Kind()))
	}
}

// namedFields turns struct fields into WIT named-fields, adding each field's
// type to the shared graph.
func namedFields(g *graphBuilder, fs []fieldInfo) []common.NamedField {
	out := make([]common.NamedField, 0, len(fs))
	for _, f := range fs {
		out = append(out, common.NamedField{
			Name:   f.name,
			Source: common.MakeFieldSourceUserSupplied(),
			Schema: g.add(f.typ),
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
		if m.outType != nil {
			out = common.MakeOutputSchemaSingle(g.add(m.outType))
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
