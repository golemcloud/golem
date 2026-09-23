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

// Package schema works with schema graphs and the values they describe, for
// code that learns its types at runtime rather than from Go declarations.
//
// The rest of the SDK goes the other way: a Go type is reflected into a schema
// and its values are encoded by a compiled codec. That works when the caller
// knows the agent it is calling. Reflection, dynamic invocation and tool
// discovery do not: they receive a schema from the host and must validate a
// value against it, render it as JSON, or describe it to a user. Those are the
// operations here.
//
// A [Ref] is one type within a graph — the graph's root, or any node inside it
// (a tool selects many roots out of one graph). It is a view, not a copy: the
// graph behind it is shared and must not be mutated.
package schema

import (
	"fmt"
	"strings"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Ref is a schema type: a graph plus the node in it that this ref denotes.
type Ref struct {
	graph types.SchemaGraph
	root  int32
}

// NewRef returns a ref to the graph's own root.
func NewRef(graph types.SchemaGraph) Ref {
	return Ref{graph: graph, root: graph.Root}
}

// NewRefAt returns a ref to one node of the graph, for a sub-schema such as a
// single tool command's input.
func NewRefAt(graph types.SchemaGraph, root int32) Ref {
	return Ref{graph: graph, root: root}
}

// Graph returns the underlying graph. Treat it as read-only: refs, values and
// clients share it.
func (r Ref) Graph() types.SchemaGraph { return r.graph }

// Root returns the index of the node this ref denotes.
func (r Ref) Root() int32 { return r.root }

// WithRoot returns a ref to another node of the same graph.
func (r Ref) WithRoot(root int32) Ref { return Ref{graph: r.graph, root: root} }

// node resolves an index to its type body, following ref-type indirections to
// the definition they name. Returns the resolved body and its index.
func (r Ref) node(idx int32) (types.SchemaTypeBody, int32, error) {
	seen := 0
	for {
		if idx < 0 || int(idx) >= len(r.graph.TypeNodes) {
			return types.SchemaTypeBody{}, 0, fmt.Errorf("type node index %d is out of range (%d nodes)", idx, len(r.graph.TypeNodes))
		}
		body := r.graph.TypeNodes[idx].Body
		if body.Tag() != types.SchemaTypeBodyRefType {
			return body, idx, nil
		}
		def := body.RefType()
		if def < 0 || int(def) >= len(r.graph.Defs) {
			return types.SchemaTypeBody{}, 0, fmt.Errorf("ref-type target %d is out of range (%d defs)", def, len(r.graph.Defs))
		}
		idx = r.graph.Defs[def].Body
		// A ref chain longer than the node count is a cycle with no body.
		seen++
		if seen > len(r.graph.TypeNodes)+1 {
			return types.SchemaTypeBody{}, 0, fmt.Errorf("ref-type chain does not terminate")
		}
	}
}

// ContainsStream reports whether the schema holds a stream anywhere, directly
// or through a named definition. A method whose input or output contains a
// stream cannot be triggered or scheduled: those return before the stream is
// consumed, and the stream would be dropped unread.
func (r Ref) ContainsStream() bool {
	visited := make(map[int32]bool, len(r.graph.TypeNodes))
	return r.containsStream(r.root, visited)
}

func (r Ref) containsStream(idx int32, visited map[int32]bool) bool {
	if visited[idx] {
		return false
	}
	visited[idx] = true

	body, _, err := r.node(idx)
	if err != nil {
		return false
	}
	switch body.Tag() {
	case types.SchemaTypeBodyStreamType:
		return true
	case types.SchemaTypeBodyFutureType:
		if inner := body.FutureType(); inner.IsSome() {
			return r.containsStream(inner.Some(), visited)
		}
		return false
	}
	for _, child := range r.children(body) {
		if r.containsStream(child, visited) {
			return true
		}
	}
	return false
}

// children returns the indices of the type nodes a composite body refers to.
func (r Ref) children(body types.SchemaTypeBody) []int32 {
	switch body.Tag() {
	case types.SchemaTypeBodyRecordType:
		fields := body.RecordType()
		out := make([]int32, 0, len(fields))
		for _, f := range fields {
			out = append(out, f.Body)
		}
		return out
	case types.SchemaTypeBodyVariantType:
		cases := body.VariantType()
		out := make([]int32, 0, len(cases))
		for _, c := range cases {
			if c.Payload.IsSome() {
				out = append(out, c.Payload.Some())
			}
		}
		return out
	case types.SchemaTypeBodyUnionType:
		branches := body.UnionType().Branches
		out := make([]int32, 0, len(branches))
		for _, b := range branches {
			out = append(out, b.Body)
		}
		return out
	case types.SchemaTypeBodyTupleType:
		return body.TupleType()
	case types.SchemaTypeBodyListType:
		return []int32{body.ListType()}
	case types.SchemaTypeBodyFixedListType:
		return []int32{body.FixedListType().Element}
	case types.SchemaTypeBodyMapType:
		spec := body.MapType()
		return []int32{spec.Key, spec.Value}
	case types.SchemaTypeBodyOptionType:
		return []int32{body.OptionType()}
	case types.SchemaTypeBodyResultType:
		spec := body.ResultType()
		var out []int32
		if spec.Ok.IsSome() {
			out = append(out, spec.Ok.Some())
		}
		if spec.Err.IsSome() {
			out = append(out, spec.Err.Some())
		}
		return out
	case types.SchemaTypeBodySecretType:
		return []int32{body.SecretType().Inner}
	case types.SchemaTypeBodyStreamType:
		if inner := body.StreamType(); inner.IsSome() {
			return []int32{inner.Some()}
		}
		return nil
	case types.SchemaTypeBodyFutureType:
		if inner := body.FutureType(); inner.IsSome() {
			return []int32{inner.Some()}
		}
		return nil
	default:
		return nil
	}
}

// TypeName renders the schema as a short, human-readable type expression, for
// error messages and for describing a discovered agent to a user. It is a
// display form, not a parse target.
func (r Ref) TypeName() string {
	var b strings.Builder
	r.writeTypeName(&b, r.root, make(map[int32]bool))
	return b.String()
}

func (r Ref) writeTypeName(b *strings.Builder, idx int32, seen map[int32]bool) {
	// A named definition prints as its name, which also breaks recursion.
	if idx >= 0 && int(idx) < len(r.graph.TypeNodes) {
		if raw := r.graph.TypeNodes[idx].Body; raw.Tag() == types.SchemaTypeBodyRefType {
			def := raw.RefType()
			if def >= 0 && int(def) < len(r.graph.Defs) {
				b.WriteString(r.graph.Defs[def].Id)
				return
			}
		}
	}
	if seen[idx] {
		b.WriteString("...")
		return
	}
	seen[idx] = true
	defer delete(seen, idx)

	body, _, err := r.node(idx)
	if err != nil {
		b.WriteString("?")
		return
	}
	switch body.Tag() {
	case types.SchemaTypeBodyBoolType:
		b.WriteString("bool")
	case types.SchemaTypeBodyS8Type:
		b.WriteString("s8")
	case types.SchemaTypeBodyS16Type:
		b.WriteString("s16")
	case types.SchemaTypeBodyS32Type:
		b.WriteString("s32")
	case types.SchemaTypeBodyS64Type:
		b.WriteString("s64")
	case types.SchemaTypeBodyU8Type:
		b.WriteString("u8")
	case types.SchemaTypeBodyU16Type:
		b.WriteString("u16")
	case types.SchemaTypeBodyU32Type:
		b.WriteString("u32")
	case types.SchemaTypeBodyU64Type:
		b.WriteString("u64")
	case types.SchemaTypeBodyF32Type:
		b.WriteString("f32")
	case types.SchemaTypeBodyF64Type:
		b.WriteString("f64")
	case types.SchemaTypeBodyCharType:
		b.WriteString("char")
	case types.SchemaTypeBodyStringType:
		b.WriteString("string")
	case types.SchemaTypeBodyTextType:
		b.WriteString("text")
	case types.SchemaTypeBodyBinaryType:
		b.WriteString("binary")
	case types.SchemaTypeBodyPathType:
		b.WriteString("path")
	case types.SchemaTypeBodyUrlType:
		b.WriteString("url")
	case types.SchemaTypeBodyDatetimeType:
		b.WriteString("datetime")
	case types.SchemaTypeBodyDurationType:
		b.WriteString("duration")
	case types.SchemaTypeBodyQuantityType:
		b.WriteString("quantity")
	case types.SchemaTypeBodyQuotaTokenType:
		b.WriteString("quota-token")
	case types.SchemaTypeBodyPermissionCardType:
		b.WriteString("permission-card")
	case types.SchemaTypeBodyEnumType:
		b.WriteString("enum{")
		b.WriteString(strings.Join(body.EnumType(), ", "))
		b.WriteString("}")
	case types.SchemaTypeBodyFlagsType:
		b.WriteString("flags{")
		b.WriteString(strings.Join(body.FlagsType(), ", "))
		b.WriteString("}")
	case types.SchemaTypeBodyRecordType:
		b.WriteString("record{")
		for i, f := range body.RecordType() {
			if i > 0 {
				b.WriteString(", ")
			}
			b.WriteString(f.Name)
			b.WriteString(": ")
			r.writeTypeName(b, f.Body, seen)
		}
		b.WriteString("}")
	case types.SchemaTypeBodyVariantType:
		b.WriteString("variant{")
		for i, c := range body.VariantType() {
			if i > 0 {
				b.WriteString(", ")
			}
			b.WriteString(c.Name)
			if c.Payload.IsSome() {
				b.WriteString("(")
				r.writeTypeName(b, c.Payload.Some(), seen)
				b.WriteString(")")
			}
		}
		b.WriteString("}")
	case types.SchemaTypeBodyUnionType:
		b.WriteString("union{")
		for i, branch := range body.UnionType().Branches {
			if i > 0 {
				b.WriteString(", ")
			}
			r.writeTypeName(b, branch.Body, seen)
		}
		b.WriteString("}")
	case types.SchemaTypeBodyTupleType:
		b.WriteString("tuple<")
		for i, elem := range body.TupleType() {
			if i > 0 {
				b.WriteString(", ")
			}
			r.writeTypeName(b, elem, seen)
		}
		b.WriteString(">")
	case types.SchemaTypeBodyListType:
		b.WriteString("list<")
		r.writeTypeName(b, body.ListType(), seen)
		b.WriteString(">")
	case types.SchemaTypeBodyFixedListType:
		spec := body.FixedListType()
		fmt.Fprintf(b, "list<")
		r.writeTypeName(b, spec.Element, seen)
		fmt.Fprintf(b, ", %d>", spec.Length)
	case types.SchemaTypeBodyMapType:
		spec := body.MapType()
		b.WriteString("map<")
		r.writeTypeName(b, spec.Key, seen)
		b.WriteString(", ")
		r.writeTypeName(b, spec.Value, seen)
		b.WriteString(">")
	case types.SchemaTypeBodyOptionType:
		b.WriteString("option<")
		r.writeTypeName(b, body.OptionType(), seen)
		b.WriteString(">")
	case types.SchemaTypeBodyResultType:
		spec := body.ResultType()
		b.WriteString("result<")
		if spec.Ok.IsSome() {
			r.writeTypeName(b, spec.Ok.Some(), seen)
		} else {
			b.WriteString("_")
		}
		b.WriteString(", ")
		if spec.Err.IsSome() {
			r.writeTypeName(b, spec.Err.Some(), seen)
		} else {
			b.WriteString("_")
		}
		b.WriteString(">")
	case types.SchemaTypeBodySecretType:
		b.WriteString("secret<")
		r.writeTypeName(b, body.SecretType().Inner, seen)
		b.WriteString(">")
	case types.SchemaTypeBodyStreamType:
		b.WriteString("stream<")
		if inner := body.StreamType(); inner.IsSome() {
			r.writeTypeName(b, inner.Some(), seen)
		} else {
			b.WriteString("_")
		}
		b.WriteString(">")
	case types.SchemaTypeBodyFutureType:
		b.WriteString("future<")
		if inner := body.FutureType(); inner.IsSome() {
			r.writeTypeName(b, inner.Some(), seen)
		} else {
			b.WriteString("_")
		}
		b.WriteString(">")
	default:
		fmt.Fprintf(b, "unknown(%d)", body.Tag())
	}
}
