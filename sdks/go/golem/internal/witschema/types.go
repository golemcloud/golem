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

// Package witschema converts between the guest SDK's generated WebAssembly
// bindings and the shared model in sdks/go/core.
//
// The two describe the same thing in different shapes. The WIT form is flat:
// one pool of nodes addressed by index, which is what a component-model record
// can carry. The core form is recursive, with references naming a definition by
// its string id, which is what the REST surface uses and what a Go program can
// read without an index in hand.
//
// This package must live inside the guest SDK: core is a separate module, and
// Go's internal rule stops it importing the generated bindings.
//
// Conversion is total in both directions rather than lazy. A value is built one
// piece at a time, and if a later field fails, the host-managed handles already
// taken for the earlier ones are out of the platform's hands — only a single
// owner of the whole tree can give them back. The cost lands on the JSON,
// reflection and dynamic paths, which already move a whole graph across the
// host boundary; the SDK's own hot path keeps using the generated types
// directly.
package witschema

import (
	"fmt"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Converted is a graph in the core form together with the index side table the
// WIT form needs.
//
// Reflection selects sub-schemas by WIT node index — a method's output, a
// tool's result, each parameter — and the recursive form has no indices at all.
// Keeping the mapping is what lets those callers stay on indices while
// everything downstream works in core terms.
type Converted struct {
	Graph   core.SchemaGraph
	byIndex map[int32]core.SchemaType
}

// At resolves a WIT node index to the core type it became.
func (c Converted) At(idx int32) (core.SchemaType, error) {
	t, ok := c.byIndex[idx]
	if !ok {
		return core.SchemaType{}, fmt.Errorf("golem: type node index %d is not part of this graph", idx)
	}
	return t, nil
}

// Ref resolves a WIT node index to a core reference into the converted graph.
func (c Converted) Ref(idx int32) (core.Ref, error) {
	t, err := c.At(idx)
	if err != nil {
		return core.Ref{}, err
	}
	return core.NewRefAt(c.Graph, t), nil
}

// GraphToCore converts a generated graph to the shared model.
func GraphToCore(g types.SchemaGraph) (Converted, error) {
	c := &converter{wit: g, byIndex: map[int32]core.SchemaType{}}

	defs := make([]core.SchemaTypeDef, 0, len(g.Defs))
	for _, d := range g.Defs {
		body, err := c.node(d.Body)
		if err != nil {
			return Converted{}, err
		}
		def := core.SchemaTypeDef{Id: d.Id, Body: body}
		if d.Name.IsSome() {
			name := d.Name.Some()
			def.Name = &name
		}
		defs = append(defs, def)
	}

	root, err := c.node(g.Root)
	if err != nil {
		return Converted{}, err
	}
	return Converted{
		Graph:   core.SchemaGraph{Defs: defs, Root: root},
		byIndex: c.byIndex,
	}, nil
}

type converter struct {
	wit     types.SchemaGraph
	byIndex map[int32]core.SchemaType
	// depth bounds the walk. A ref node terminates recursion on its own, but a
	// malformed graph can still nest without end.
	depth int
}

const maxDepth = 512

// node converts one WIT node. A ref becomes a core reference and is NOT
// followed: the definition it names is converted in its own right, which is
// what keeps a recursive type finite.
func (c *converter) node(idx int32) (core.SchemaType, error) {
	if t, done := c.byIndex[idx]; done {
		return t, nil
	}
	if idx < 0 || int(idx) >= len(c.wit.TypeNodes) {
		return core.SchemaType{}, fmt.Errorf("golem: type node index %d is out of range (%d nodes)",
			idx, len(c.wit.TypeNodes))
	}
	if c.depth >= maxDepth {
		return core.SchemaType{}, fmt.Errorf("golem: schema nests deeper than %d levels", maxDepth)
	}
	c.depth++
	defer func() { c.depth-- }()

	n := c.wit.TypeNodes[idx]
	body, err := c.body(n.Body)
	if err != nil {
		return core.SchemaType{}, err
	}
	t := core.SchemaType{Body: body, Metadata: metadataToCore(n.Metadata)}
	c.byIndex[idx] = t
	return t, nil
}

// child converts a nested node.
func (c *converter) child(idx int32) (core.SchemaType, error) { return c.node(idx) }

// optChild converts an optional nested node.
func (c *converter) optChild(o witTypes.Option[int32]) (*core.SchemaType, error) {
	if o.IsNone() {
		return nil, nil
	}
	t, err := c.child(o.Some())
	if err != nil {
		return nil, err
	}
	return &t, nil
}
